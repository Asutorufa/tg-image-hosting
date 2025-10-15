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
    if let Ok(v) = env.var(key) {
        return v.to_string();
    }

    String::new()
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

    let host = &match req.url() {
        Ok(v) => match v.host() {
            Some(v) => v.to_string(),
            None => return Response::error("no host", 500),
        },
        Err(e) => return Response::error(e.to_string(), 500),
    };

    let router = Router::with_data(match init_bot(&env) {
        Ok(v) => v,
        Err(e) => {
            error!("{}", e);
            return Response::ok(format!("Error: {}", e));
        }
    })
    .on_async("/tgbot/register", async |_req, ctx| {
        let url = format!("https://{}/tgbot", host);

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

        match ctx.data.handle(host, update).await {
            Ok(_) => info!("Update was handled by bot."),
            Err(e) => error!("Update was not handled by bot: {}", e),
        };

        return Response::ok("ok");
    })
    .get_async("/f/:file_id", async |_, ctx| {
        let cache = Cache::default();

        let (file_id, ext) = match ctx.param("file_id") {
            Some(v) => {
                let p = Path::new(v);
                (
                    p.file_stem().unwrap_or_default().to_string_lossy(),
                    p.extension().unwrap_or_default().to_string_lossy(),
                )
            }
            None => return Response::error("file_id not found", 400),
        };

        let url = format!("https://{}/f/{}.{}", host, file_id, ext);

        let cache_key = Request::new(&url, Method::Get)?;

        if let Ok(Some(v)) = cache.get(CacheKey::from(&cache_key), true).await {
            return Ok(v);
        }

        let opt = ctx.data.clone();

        info!("file id: {}, ext: {}", file_id, ext);

        let url = match opt.get_file_url(file_id).await {
            Ok(v) => v,
            Err(e) => return Response::error(e.to_string(), 500),
        };

        let stream = match download(url).await {
            Ok(v) => v,
            Err(e) => return Response::error(e.to_string(), 500),
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

            let resp = ResponseBuilder::new()
                .with_headers(headers.clone())
                .body(ResponseBody::Stream(s2));

            if let Err(e) = cache.put(CacheKey::from(&cache_key), resp).await {
                error!("put cache error: {}", e);
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

async fn download(url: String) -> std::result::Result<ReadableStream, Error> {
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

    let response = Fetch::Request(request).send().await?;

    let stream = match &response.body() {
        ResponseBody::Stream(edge_request) => edge_request.clone(),
        _ => return Err(Error::RustError("body is not streamable".into())),
    };

    Ok(stream)
}
