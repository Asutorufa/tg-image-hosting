pub mod consolelog;
pub mod tg;

use crate::tg::D1;
use frankenstein::client_reqwest::Bot;
use frankenstein::updates::Update;
use futures_util::TryStreamExt;
use log::info;
use log::{debug, error};
use std::ops::Deref;
use std::path::Path;
use std::sync::Arc;
use std::sync::Once;
use worker::*;

static INIT: Once = Once::new();

fn get_string_from_env(env: Arc<Env>, key: &str) -> String {
    match env.var(key) {
        Ok(v) => match v.as_ref().as_string() {
            Some(v) => v,
            None => "".to_string(),
        },
        _ => "".to_string(),
    }
}

#[derive(Clone)]
pub struct RunOpt {
    pub matainer: i64,
    pub d1: D1,
    pub bot: Arc<Bot>,
    pub bot_token: String,
}

async fn get_opt(env: Arc<Env>) -> Result<RunOpt> {
    console_error_panic_hook::set_once();
    INIT.call_once(|| {
        match consolelog::init_with_level(log::Level::Info) {
            Err(e) => console_error!("Failed to init console log: {}", e),
            _ => console_log!("Console log initialized"),
        };
    });

    let token = get_string_from_env(env.clone(), "TELEGRAM_TOKEN");

    let maintainer_id = get_string_from_env(env.clone(), "MAINTAINER_ID")
        .parse::<i64>()
        .unwrap_or(0);

    let d1 = tg::D1::new(Arc::new(env.d1("DB")?));

    Ok(RunOpt {
        bot: Arc::new(Bot::new(&token)),
        matainer: maintainer_id,
        d1,
        bot_token: token,
    })
}

#[event(fetch)]
async fn main(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    if req.method() == Method::Options {
        return Response::ok("");
    }

    let env = Arc::new(env);
    let opt = match get_opt(env.clone()).await {
        Ok(v) => v,
        Err(e) => {
            error!("{}", e);
            return Response::ok(format!("Error: {}", e));
        }
    };

    let mut router = Router::new();

    router = router.on_async("/tgbot/register", async |req, _ctx| {
        let url = format!("https://{}/tgbot", req.url()?.host().unwrap());

        match tg::set_webhook(&opt.bot, url.as_ref(), opt.matainer).await {
            Ok(_) => Response::ok(format!("register webhook to {} successful", url)),
            Err(e) => Response::error(e.to_string(), 500),
        }
    });

    router = router.on_async("/d1/create_table", async |_, _ctx| {
        match opt.d1.init().await {
            Ok(_) => Response::ok(format!("create table [words] successful")),
            Err(e) => Response::error(e.to_string(), 500),
        }
    });

    router = router.post_async("/tgbot", async |mut req, _ctx| {
        let update = req.json::<Update>().await?;

        debug!("body: {:?}", update);

        let host = req.url()?.host().unwrap().to_string();

        return match tg::handle(opt.d1.clone(), &opt.bot, host, update).await {
            Ok(_) => {
                debug!("Update was handled by bot.");
                Response::ok("Update was handled by bot.")
            }
            Err(e) => {
                error!("Update was not handled by bot: {}", e);
                Response::ok(format!("Update was not handled by bot: {}", e))
            }
        };
    });

    router = router.get_async("/f/:file_id", async |req: Request, ctx| {
        let cache = Cache::default();

        match cache.get(CacheKey::from(&req), true).await {
            Ok(v) => match v {
                Some(v) => return Ok(v),
                None => {}
            },
            _ => {}
        };

        let file_id = match ctx.param("file_id") {
            Some(v) => Path::new(v),
            None => return Response::error("file_id not found", 400),
        };

        info!(
            "file id: {}, ext: {}",
            file_id.file_stem().unwrap_or_default().to_string_lossy(),
            file_id.extension().unwrap_or_default().to_string_lossy()
        );

        let file = match tg::get_file(
            opt.d1.clone(),
            opt.bot.clone(),
            opt.bot_token.clone(),
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

        let headers = Headers::new();
        let _ = headers.set("Cache-Control", "public, max-age=31536000");
        let resp = ResponseBuilder::new()
            .with_headers(headers)
            .from_stream(file.map_err(|e| worker::Error::from(e.to_string())))?;

        match cache.put(CacheKey::from(&req), resp).await {
            Ok(_) => {}
            Err(e) => return Response::error(e.to_string(), 500),
        }

        match cache.get(CacheKey::from(&req), true).await {
            Ok(v) => match v {
                Some(v) => return Ok(v),
                None => return Response::error("file not found from cache", 500),
            },
            _ => return Response::error("file not found from cache", 500),
        }
    });

    let resp = match router.run(req, env.deref().clone()).await {
        Ok(v) => v,
        Err(e) => return Response::error(e.to_string(), 500),
    };

    // if resp.status_code() == 404 {
    //     return env
    //         .assets("ASSETS")?
    //         .fetch_request(req.clone().unwrap())
    //         .await;
    // }

    Ok(resp)
}
