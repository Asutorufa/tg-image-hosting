use crate::tg::TgBot;
use frankenstein::updates::Update;
use log::error;
use log::info;
use std::path::Path;
use std::sync::Arc;
use wasm_bindgen::JsCast;
use web_sys::ReadableStream;
use worker::*;

pub struct Handler {
    host: String,
    pub r2: Option<Bucket>,
    bot: Arc<TgBot>,
    ctx: Arc<Context>,
    pub cache: Arc<Cache>,
}

impl Handler {
    pub fn new(host: String, r2: Option<Bucket>, bot: Arc<TgBot>, ctx: Arc<Context>) -> Self {
        Self {
            host,
            r2,
            bot,
            ctx,
            cache: Arc::new(Cache::default()),
        }
    }

    pub async fn put_to_r2(
        &self,
        key: &str,
        data: ReadableStream,
    ) -> std::result::Result<ReadableStream, crate::error::Error> {
        if let Some(v) = &self.r2 {
            let (s1, s2) = splite_readable_stream(data)?;

            let key = key.to_string();
            let v = v.clone();

            self.ctx.wait_until(async move {
                if let Err(e) = v.put(key, s2).execute().await {
                    error!("Put file error: {:#?}", e);
                }
            });

            Ok(s1)
        } else {
            Ok(data)
        }
    }

    pub async fn get_cache(&self, key: &Request) -> Option<Response> {
        if let Ok(v) = self.cache.get(CacheKey::from(key), true).await {
            v
        } else {
            None
        }
    }

    pub async fn put_cache(
        &self,
        key: Request,
        data: ReadableStream,
    ) -> std::result::Result<ReadableStream, crate::error::Error> {
        let (s1, s2) = splite_readable_stream(data)?;

        let cache = self.cache.clone();

        self.ctx.wait_until(async move {
            let resp = ResponseBuilder::new()
                .with_header("Cache-Control", "public, max-age=31536000")
                .unwrap_or_else(|_| ResponseBuilder::new())
                .body(ResponseBody::Stream(s2));

            if let Err(e) = cache.put(CacheKey::from(&key), resp).await {
                error!("put cache error: {}", e);
            };
        });

        Ok(s1)
    }

    async fn get_file(
        &self,
        file_id: &str,
        ext: &str,
    ) -> std::result::Result<ReadableStream, crate::error::Error> {
        let (url, file_uniq_id) = self.bot.get_file_url(file_id).await?;

        let r2_key = format!("{}.{}", file_uniq_id, ext);

        // get from r2 cache first
        if let Some(r2) = self.r2.as_ref()
            && let Ok(Some(v)) = r2.get(&r2_key).execute().await
            && let Some(body) = v.body()
            && let Ok(ResponseBody::Stream(s)) = body.response_body()
        {
            info!("use r2 cache");
            return Ok(s);
        }

        info!("download from raw");

        self.put_to_r2(&r2_key, download(url).await?).await
    }

    pub async fn download(
        &self,
        _: Request,
        ctx: RouteContext<()>,
    ) -> std::result::Result<Response, crate::error::Error> {
        let file_name = match ctx.param("file_id") {
            Some(v) => v,
            None => return Err(crate::error::Error("file name is not found".into())),
        };

        let p = Path::new(file_name);
        let file_id = p.file_stem().unwrap_or_default().to_string_lossy();
        let ext = p.extension().unwrap_or_default().to_string_lossy();

        let url = format!("https://{}/f/{}.{}", self.host, file_id, ext);

        let cache_key = Request::new(&url, Method::Get)?;

        if let Some(v) = self.get_cache(&cache_key).await {
            return Ok(v);
        }

        let stream = self.get_file(file_id.as_ref(), ext.as_ref()).await?;

        let stream = self.put_cache(cache_key, stream).await?;

        Ok(ResponseBuilder::new()
            .with_header("Cache-Control", "public, max-age=31536000")?
            .body(ResponseBody::Stream(stream)))
    }

    pub async fn telegram(
        &self,
        mut req: Request,
        _ctx: RouteContext<()>,
    ) -> std::result::Result<(), crate::error::Error> {
        let update = req.json::<Update>().await?;
        info!("body: {:?}", update);
        self.bot.handle(&self.host, update).await?;
        Ok(())
    }

    pub async fn register(
        &self,
        _: Request,
        _ctx: RouteContext<()>,
    ) -> std::result::Result<(), crate::error::Error> {
        let url = format!("https://{}/tgbot", self.host);
        self.bot.set_webhook(url.as_ref()).await?;
        Ok(())
    }

    pub async fn init_database(
        &self,
        _: Request,
        _ctx: RouteContext<()>,
    ) -> std::result::Result<(), crate::error::Error> {
        self.bot.d1.init().await?;
        Ok(())
    }

    pub fn github_page(_: Request, _: RouteContext<()>) -> Result<Response> {
        return Response::redirect(
            Url::parse("https://github.com/Asutorufa/tg-image-hosting").unwrap(),
        );
    }
}

fn splite_readable_stream(
    r: ReadableStream,
) -> std::result::Result<(ReadableStream, ReadableStream), crate::error::Error> {
    let tee_off = r.tee();
    Ok((tee_off.get(0).dyn_into()?, tee_off.get(1).dyn_into()?))
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
        ResponseBody::Stream(edge_request) => edge_request,
        _ => return Err(Error::RustError("body is not streamable".into())),
    };

    Ok(stream.clone())
}
