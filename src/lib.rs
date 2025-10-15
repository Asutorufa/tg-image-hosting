pub mod consolelog;
pub mod d1;
pub mod error;
pub mod tg;

use crate::tg::TgBot;
use frankenstein::updates::Update;
use log::error;
use log::info;
use std::path::Path;
use std::sync::Arc;
use wasm_bindgen::JsCast;
use web_sys::ReadableStream;
use worker::*;

fn get_string_from_env(env: &Env, key: &str) -> String {
    match env.var(key) {
        Ok(v) => match v.as_ref().as_string() {
            Some(v) => v,
            None => "".to_string(),
        },
        _ => "".to_string(),
    }
}

// Multiple calls to `init` will cause a panic as a tracing subscriber is already set.
// So we use the `start` event to initialize our tracing subscriber when the worker starts.
#[event(start)]
fn start() {
    console_error_panic_hook::set_once();
    match consolelog::init_with_level(log::Level::Info) {
        Err(e) => console_error!("Failed to init console log: {}", e),
        _ => console_log!("Console log initialized"),
    };
}

fn init_bot(env: &Env) -> Result<TgBot> {
    let token = get_string_from_env(env, "TELEGRAM_TOKEN");

    let maintainer_id = get_string_from_env(env, "MAINTAINER_ID")
        .parse::<i64>()
        .unwrap_or(0);

    let d1 = d1::D1::new(Arc::new(env.d1("DB")?));

    Ok(TgBot::new(d1, maintainer_id, token))
}

#[event(fetch)]
async fn main(req: Request, env: Env, global_ctx: Context) -> Result<Response> {
    if req.method() == Method::Options {
        return Response::ok("");
    }

    let router = Router::with_data(match init_bot(&env) {
        Ok(v) => v,
        Err(e) => {
            error!("{}", e);
            return Response::ok(format!("Error: {}", e));
        }
    })
    .on_async("/tgbot/register", async |req, ctx| {
        let url = format!("https://{}/tgbot", req.url()?.host().unwrap());

        match ctx.data.set_webhook(url.as_ref()).await {
            Ok(_) => Response::ok(format!("register webhook to {} successful", url)),
            Err(e) => Response::error(e.to_string(), 500),
        }
    })
    .on_async("/d1/create_table", async |_, ctx| {
        match ctx.data.d1.init().await {
            Ok(_) => Response::ok(format!("create table [words] successful")),
            Err(e) => Response::error(e.to_string(), 500),
        }
    })
    .post_async("/tgbot", async |mut req, ctx| {
        let update = match req.json::<Update>().await {
            Ok(v) => v,
            Err(e) => return Response::error(e.to_string(), 400),
        };

        info!("body: {:?}", update);

        let host = req.url()?.host().unwrap().to_string();

        return match ctx.data.handle(host, update).await {
            Ok(_) => {
                info!("Update was handled by bot.");
                Response::ok("Update was handled by bot.")
            }
            Err(e) => {
                error!("Update was not handled by bot: {}", e);
                Response::ok(format!("Update was not handled by bot: {}", e))
            }
        };
    })
    .get_async("/f/:file_id", async |req: Request, ctx| {
        let cache = Cache::default();

        match cache.get(CacheKey::from(&req), true).await {
            Ok(v) => match v {
                Some(v) => return Ok(v),
                None => {}
            },
            _ => {}
        };

        let opt = ctx.data.clone();

        let file_id = match ctx.param("file_id") {
            Some(v) => Path::new(v),
            None => return Response::error("file_id not found", 400),
        };

        info!(
            "file id: {}, ext: {}",
            file_id.file_stem().unwrap_or_default().to_string_lossy(),
            file_id.extension().unwrap_or_default().to_string_lossy()
        );

        let url = match opt
            .get_file_url(
                file_id
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string(),
            )
            .await
        {
            Ok(v) => v,
            Err(e) => return Response::error(e.to_string(), 500),
        };

        let file = match download(url).await {
            Ok(v) => v,
            Err(e) => return Response::error(e.to_string(), 500),
        };

        let stream = match &file.body() {
            ResponseBody::Stream(edge_request) => edge_request.clone(),
            _ => return Err(Error::RustError("body is not streamable".into())),
        };

        let tee_off = stream.tee();

        let s1: ReadableStream = match tee_off.get(0).dyn_into() {
            Ok(v) => v,
            Err(e) => return Response::error(e.as_string().unwrap_or_default(), 500),
        };

        let headers = Headers::new();
        let _ = headers.set("Cache-Control", "public, max-age=31536000");

        let resp = ResponseBuilder::new()
            .with_headers(headers.clone())
            .body(ResponseBody::Stream(s1));

        global_ctx.wait_until(async move {
            let s2: ReadableStream = match tee_off.get(1).dyn_into() {
                Ok(v) => v,
                Err(e) => {
                    error!("get stream error: {}", e.as_string().unwrap_or_default());
                    return;
                }
            };

            match cache
                .put(
                    CacheKey::from(&req),
                    ResponseBuilder::new()
                        .with_headers(headers.clone())
                        .body(ResponseBody::Stream(s2)),
                )
                .await
            {
                Ok(_) => {}
                Err(e) => error!("put cache error: {}", e),
            };
        });

        Ok(resp)
    })
    .on_async("/", async |_req, _ctx| {
        return Response::redirect(
            Url::parse("https://github.com/Asutorufa/tg-image-hosting").unwrap(),
        );
    })
    .or_else_any_method_async("/*catchall", async |_req, _ctx| {
        return Response::redirect(
            Url::parse("https://github.com/Asutorufa/tg-image-hosting").unwrap(),
        );
    });

    Ok(match router.run(req, env).await {
        Ok(v) => v,
        Err(e) => return Response::error(e.to_string(), 500),
    })
}

async fn download(url: String) -> std::result::Result<Response, Error> {
    let request = Request::new_with_init(
        url.as_str(),
        &RequestInit {
            method: Method::Get,
            cf: CfProperties {
                cache_ttl: Some(31536000),
                cache_everything: Some(true),
                ..CfProperties::default()
            },
            ..RequestInit::default()
        },
    )?;

    let mut response = Fetch::Request(request).send().await?;

    let _ = response
        .headers_mut()
        .set("Cache-Control", "max-age=31536000");

    Ok(response)
}
