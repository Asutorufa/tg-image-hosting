pub mod consolelog;
pub mod d1;
pub mod error;
pub mod handler;
pub mod tg;

use crate::handler::Handler;
use crate::tg::TgBot;
use log::error;
use log::info;
use std::sync::Arc;
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

fn init_bot(env: &Env) -> Result<Arc<TgBot>> {
    let token = get_string_from_env(env, "TELEGRAM_TOKEN");

    let maintainer_id = get_string_from_env(env, "MAINTAINER_ID")
        .parse::<i64>()
        .unwrap_or(0);

    let d1 = d1::D1::new(Arc::new(env.d1("DB")?));

    Ok(Arc::new(TgBot::new(d1, maintainer_id, token)))
}

#[event(fetch)]
async fn main(req: Request, env: Env, ctx: Context) -> Result<Response> {
    if req.method() == Method::Options {
        return Response::ok("");
    }

    let host = if let Ok(v) = req.url()
        && let Some(v) = v.host()
    {
        v.to_string()
    } else {
        return Response::error("Host not found", 400);
    };

    let bot = match init_bot(&env) {
        Ok(v) => v,
        Err(e) => return Response::ok(format!("Error: {}", e)),
    };

    let handler = Handler::new(host.to_string(), env.bucket("R2").ok(), bot, Arc::new(ctx));

    let router = Router::new()
        .on_async("/tgbot/register", async |_req: Request, ctx| {
            handler.register(_req, ctx).await.map_or_else(
                |e| e.to_response(),
                |_| Response::ok("register webhook successful"),
            )
        })
        .on_async("/d1/create_table", async |req, ctx| {
            handler.init_database(req, ctx).await.map_or_else(
                |e| e.to_response(),
                |_| Response::ok("init database successful"),
            )
        })
        .post_async("/tgbot", async |req, ctx| {
            match handler.telegram(req, ctx).await {
                Ok(_) => info!("Update was handled by bot."),
                Err(e) => error!("Update was not handled by bot: {}", e),
            };
            return Response::ok("ok");
        })
        .get_async("/f/:file_id", async |req, ctx| {
            match handler.download(req, ctx).await {
                Ok(v) => Ok(v),
                Err(e) => e.to_response(),
            }
        })
        .on("/", Handler::github_page)
        .or_else_any_method("/*catchall", Handler::github_page);

    Ok(match router.run(req, env).await {
        Ok(v) => v,
        Err(e) => return Response::error(e.to_string(), 500),
    })
}
