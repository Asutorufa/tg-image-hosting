use crate::tg::TgBot;
use frankenstein::updates::Update;
use log::error;
use log::info;
use log::warn;
use std::collections::HashMap;
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
        let (url, file_uniq_id) = self.bot.get_file_url(file_id, false).await?;

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

        let stream = match download(url).await? {
            DownloadResult::Stream(v) => v,
            DownloadResult::NotFound => {
                // retry to get path
                warn!("file not found, retry to get new path");
                let (url, _) = self.bot.get_file_url(file_id, true).await?;
                match download(url).await? {
                    DownloadResult::Stream(v) => v,
                    DownloadResult::NotFound => {
                        return Err(crate::error::Error("file not found".into()));
                    }
                }
            }
        };

        self.put_to_r2(&r2_key, stream).await
    }

    pub async fn download(
        &self,
        _req: Request,
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

        // let no_cache = req
        //     .query::<HashMap<String, String>>()
        //     .unwrap_or_default()
        //     .get("no_cache")
        //     .unwrap_or(&"false".to_string())
        //     .parse::<bool>()
        //     .unwrap_or_default();

        // if !no_cache {
        if let Some(v) = self.get_cache(&cache_key).await {
            return Ok(v);
        }
        // }

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

pub enum DownloadResult {
    Stream(ReadableStream),
    NotFound,
}
async fn download(url: String) -> std::result::Result<DownloadResult, crate::error::Error> {
    let request = Request::new_with_init(
        url.as_str(),
        &RequestInit {
            method: Method::Get,
            cf: CfProperties {
                cache_ttl_by_status: Some(HashMap::from([("200-299".to_string(), 31536000)])),
                ..CfProperties::default()
            },
            ..RequestInit::default()
        },
    )?;

    let mut response = Fetch::Request(request).send().await?;

    if response.status_code() == 404 {
        return Ok(DownloadResult::NotFound);
    }

    if response.status_code() != 200 {
        return Err(crate::error::Error(format!(
            "status code is not 200, but {}, {}",
            response.status_code(),
            response.text().await?
        )));
    }

    let stream = match &response.body() {
        ResponseBody::Stream(edge_request) => edge_request,
        _ => return Err(crate::error::Error("body is not streamable".into())),
    };

    Ok(DownloadResult::Stream(stream.clone()))
}
