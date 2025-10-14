use frankenstein::client_reqwest::Bot;
use frankenstein::methods::{GetFileParams, SendMessageParams, SetWebhookParams};
use frankenstein::types::{ChatId, LinkPreviewOptions, ReplyParameters};
use frankenstein::{AsyncTelegramApi, reqwest};
use frankenstein::{
    types::{Document, Message, PhotoSize, Video},
    updates::UpdateContent,
};
use futures_core::Stream;
use log::info;
use serde::{Deserialize, Serialize};
use std::{fmt::Display, sync::Arc};
use worker::D1Database;

pub static CREATE_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS [files] (
    "file_id" TEXT PRIMARY KEY,
    "file_unique_id" TEXT,
    "thumbnail_file_id" TEXT,
    "thumbnail_file_unique_id" TEXT,
    "message_id" INTEGER,
    "user_id" INTEGER,
    "file_name" TEXT,
    "file_size" INTEGER,
    "mime_type" TEXT,
    "add_time" INTEGER,
    "update_time" INTEGER,
    "file_path" TEXT
);
"#;

pub static INSERT_FILE: &str = r#"
INSERT INTO 
    files(
        file_id,
        file_unique_id,
        thumbnail_file_id, 
        thumbnail_file_unique_id, 
        message_id, 
        user_id, 
        file_name, 
        file_size, 
        mime_type, 
        add_time, 
        update_time,
        file_path
    ) 
        VALUES
    (?, ?, ?, ?, ?, ?, ?, ?, ?, strftime('%s', 'now'), strftime('%s', 'now'), ?) 
        ON CONFLICT(file_id)
        DO UPDATE
    SET
        thumbnail_file_id = ?, 
        thumbnail_file_unique_id = ?, 
        message_id = ?, 
        user_id = ?, 
        file_name = ?, 
        file_size = ?, 
        mime_type = ?, 
        update_time = strftime('%s', 'now'), 
        file_path = ?
"#;

pub static SAVE_FILE_PATH: &str = r#"UPDATE files SET file_path = ? WHERE file_id = ?"#;

pub static SELECT_FILE: &str = r#"SELECT * FROM files WHERE file_id = ? OR file_unique_id = ?"#;

#[derive(Debug)]
pub struct Error(pub String);

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for Error {
    fn from(err: String) -> Self {
        Error(err)
    }
}

impl From<worker::Error> for Error {
    fn from(err: worker::Error) -> Self {
        Error(err.to_string())
    }
}

impl From<frankenstein::Error> for Error {
    fn from(err: frankenstein::Error) -> Self {
        Error(err.to_string())
    }
}

impl From<reqwest::Error> for Error {
    fn from(err: reqwest::Error) -> Self {
        Error(err.to_string())
    }
}

pub(super) const MARKDOWN_ESCAPE_CHARS: [char; 19] = [
    '\\', '_', '*', '[', ']', '(', ')', '~', '`', '>', '#', '+', '-', '=', '|', '{', '}', '.', '!',
];

pub fn markdown_escape(s: &str) -> String {
    s.chars().fold(String::with_capacity(s.len()), |mut s, c| {
        if MARKDOWN_ESCAPE_CHARS.contains(&c) {
            s.push('\\');
        }
        s.push(c);
        s
    })
}

pub async fn handle(
    d1: D1,
    bot: &Bot,
    host: String,
    update: frankenstein::updates::Update,
) -> Result<(), Error> {
    match update.content {
        UpdateContent::Message(msg)
        | UpdateContent::EditedMessage(msg)
        | UpdateContent::ChannelPost(msg) => {
            let chat_id = msg.chat.id;
            let msg_id = msg.message_id;

            let files = File::from_message(msg, async |f| {
                let ff = bot.get_file(&GetFileParams { file_id: f }).await?;
                match ff.result.file_path {
                    Some(p) => Ok(p),
                    None => return Err(Error("File path not found".to_string())),
                }
            })
            .await?;

            let mut response = String::new();

            match d1.save(files.clone()).await {
                Ok(_) => {
                    for f in files {
                        let ext = match f.file_path.split('.').last() {
                            Some(ext) => format!(".{}", ext),
                            None => "".to_string(),
                        };
                        response += format!("https://{}/f/{}{}\n", host, f.file_id, ext).as_str();
                        response +=
                            format!("https://{}/f/{}{}\n", host, f.file_unique_id, ext).as_str();
                    }
                }
                Err(e) => {
                    response = format!("Error: {}", e);
                }
            }

            bot.send_message(
                &SendMessageParams::builder()
                    .chat_id(ChatId::Integer(chat_id))
                    .reply_parameters(ReplyParameters::builder().message_id(msg_id).build())
                    .text(markdown_escape(response.as_str()))
                    .link_preview_options(LinkPreviewOptions::DISABLED)
                    .parse_mode(frankenstein::ParseMode::MarkdownV2)
                    .build(),
            )
            .await?;
        }

        _ => return Err(Error("no message supported".to_string())),
    };
    Ok(())
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct File {
    pub file_id: String,
    pub file_unique_id: String,
    pub thumbnail_file_id: String,
    pub thumbnail_file_unique_id: String,
    pub message_id: i32,
    pub user_id: u64,
    pub file_name: String,
    pub file_size: u64,
    pub mime_type: String,
    pub add_time: i64,
    pub update_time: i64,
    pub file_path: String,
}

impl File {
    pub fn with_message_id(mut self, message_id: i32) -> Self {
        self.message_id = message_id;
        self
    }

    pub fn with_user_id(mut self, user_id: u64) -> Self {
        self.user_id = user_id;
        self
    }

    pub fn with_file_path(mut self, file_path: String) -> Self {
        self.file_path = file_path;
        self
    }

    pub async fn from_message<F, Fut>(
        msg: Box<Message>,
        get_file_path: F,
    ) -> Result<Vec<File>, Error>
    where
        F: Fn(String) -> Fut,
        Fut: Future<Output = Result<String, Error>>,
    {
        let user_id = match msg.from {
            Some(u) => u.id,
            None => 0,
        };
        let msg_id = msg.message_id;

        let mut files = Vec::new();
        if let Some(doc) = msg.document {
            let file_id = doc.file_id.clone();
            files.push(
                File::from(doc)
                    .with_message_id(msg_id)
                    .with_user_id(user_id)
                    .with_file_path(get_file_path(file_id).await?),
            );
        }
        if let Some(photos) = msg.photo {
            match photos.last() {
                Some(photo) => {
                    let file_id = photo.file_id.clone();
                    files.push(
                        File::from(photo.clone())
                            .with_message_id(msg_id)
                            .with_user_id(user_id)
                            .with_file_path(get_file_path(file_id).await?),
                    );
                }
                _ => {}
            };
        }
        if let Some(video) = msg.video {
            let file_id = video.file_id.clone();
            files.push(
                File::from(video)
                    .with_message_id(msg_id)
                    .with_user_id(user_id)
                    .with_file_path(get_file_path(file_id).await?),
            );
        }
        Ok(files)
    }
}

impl From<Box<Video>> for File {
    fn from(v: Box<Video>) -> Self {
        let (thumbnail_file_id, thumbnail_file_unique_id) = match v.thumbnail {
            Some(t) => (t.file_id, t.file_unique_id),
            None => ("".to_string(), "".to_string()),
        };
        File {
            file_id: v.file_id,
            file_unique_id: v.file_unique_id,
            thumbnail_file_id: thumbnail_file_id,
            thumbnail_file_unique_id: thumbnail_file_unique_id,
            file_size: v.file_size.unwrap_or_default(),
            mime_type: v.mime_type.unwrap_or_default(),
            file_name: v.file_name.unwrap_or_default(),
            add_time: 0,
            update_time: 0,
            message_id: 0,
            user_id: 0,
            file_path: "".to_string(),
        }
    }
}

impl From<Box<Document>> for File {
    fn from(value: Box<Document>) -> Self {
        let (thumbnail_file_id, thumbnail_file_unique_id) = match value.thumbnail {
            Some(t) => (t.file_id, t.file_unique_id),
            None => ("".to_string(), "".to_string()),
        };
        File {
            file_id: value.file_id,
            file_unique_id: value.file_unique_id,
            thumbnail_file_id: thumbnail_file_id,
            thumbnail_file_unique_id: thumbnail_file_unique_id,
            file_size: value.file_size.unwrap_or_default(),
            mime_type: value.mime_type.unwrap_or_default(),
            file_name: value.file_name.unwrap_or_default(),
            add_time: 0,
            update_time: 0,
            message_id: 0,
            user_id: 0,
            file_path: "".to_string(),
        }
    }
}

impl From<PhotoSize> for File {
    fn from(value: PhotoSize) -> Self {
        File {
            file_id: value.file_id,
            file_unique_id: value.file_unique_id,
            thumbnail_file_id: "".to_string(),
            thumbnail_file_unique_id: "".to_string(),
            file_size: value.file_size.unwrap_or_default(),
            mime_type: "".to_string(),
            file_name: "".to_string(),
            add_time: 0,
            update_time: 0,
            message_id: 0,
            user_id: 0,
            file_path: "".to_string(),
        }
    }
}

#[derive(Clone)]
pub struct D1 {
    db: Arc<D1Database>,
}

impl D1 {
    pub fn new(db: Arc<D1Database>) -> D1 {
        D1 { db }
    }

    pub async fn init(&self) -> Result<(), Error> {
        self.db.prepare(CREATE_TABLE).run().await?;
        Ok(())
    }

    pub async fn save_file_path(&self, file_id: String, file_path: String) -> Result<(), Error> {
        let statement = self.db.prepare(SAVE_FILE_PATH);
        statement
            .clone()
            .bind(&vec![file_path.into(), file_id.into()])?
            .run()
            .await?;
        Ok(())
    }

    pub async fn save(&self, files: Vec<File>) -> Result<(), Error> {
        if files.is_empty() {
            return Ok(());
        }

        let statement = self.db.prepare(INSERT_FILE);

        let statements = files
            .into_iter()
            .map(|f| {
                statement
                    .clone()
                    .bind(&vec![
                        f.file_id.clone().into(),
                        f.file_unique_id.clone().into(),
                        f.thumbnail_file_id.clone().into(),
                        f.thumbnail_file_unique_id.clone().into(),
                        f.message_id.to_string().into(),
                        f.user_id.to_string().into(),
                        f.file_name.clone().into(),
                        f.file_size.to_string().into(),
                        f.mime_type.clone().into(),
                        f.file_path.clone().into(),
                        // on conflict
                        f.thumbnail_file_id.into(),
                        f.thumbnail_file_unique_id.into(),
                        f.message_id.to_string().into(),
                        f.user_id.to_string().into(),
                        f.file_name.into(),
                        f.file_size.to_string().into(),
                        f.mime_type.into(),
                        f.file_path.clone().into(),
                    ])
                    .unwrap()
            })
            .collect::<Vec<_>>();

        match self.db.batch(statements.clone()).await {
            Ok(_) => Ok(()),
            Err(e) => match e {
                worker::Error::D1(d1e) => {
                    if d1e.cause().contains("no such table") {
                        self.init().await?;
                        self.db.batch(statements).await?;
                        Ok(())
                    } else {
                        Err(Error(d1e.to_string()))
                    }
                }
                _ => Err(Error(e.to_string())),
            },
        }
    }

    pub async fn get(&self, file_id: String) -> Result<File, Error> {
        let statement = self.db.prepare(SELECT_FILE);
        let result = statement
            .bind(&vec![file_id.clone().into(), file_id.into()])?
            .run()
            .await?;

        if !result.error().is_none() {
            return Err(Error(result.error().unwrap().to_string()));
        }

        info!("query result: {}", result.success());

        let files = result.results::<File>()?;

        if files.is_empty() {
            return Err(Error("File not found".to_string()));
        }

        Ok(files[0].clone())
    }
}

pub async fn set_webhook(bot: &Bot, url: &str, matainer: i64) -> Result<(), Error> {
    info!("Registering webhook: {}", url);

    bot.set_webhook(&SetWebhookParams::builder().url(url).build())
        .await?;

    bot.send_message(
        &SendMessageParams::builder()
            .chat_id(ChatId::Integer(matainer))
            .text(format!("register webhook to {} successful", url))
            .link_preview_options(LinkPreviewOptions::DISABLED)
            .build(),
    )
    .await?;

    Ok(())
}

pub async fn get_file(
    d1: D1,
    bot: Arc<Bot>,

    bot_token: String,
    file_id: String,
) -> Result<impl Stream<Item = Result<bytes::Bytes, reqwest::Error>>, Error> {
    let f = d1.get(file_id.clone()).await?;

    info!("Downloading file: {}", f.file_id);

    let file_path = match f.file_path.as_str() {
        "" => {
            let f = bot
                .get_file(&GetFileParams {
                    file_id: f.file_id.clone(),
                })
                .await?;

            match f.result.file_path {
                Some(p) => {
                    d1.save_file_path(file_id.clone(), p.clone()).await?;
                    p
                }
                None => return Err(Error("File path not found".to_string())),
            }
        }
        _ => f.file_path,
    };

    info!("File path: {}", file_path);

    // https://core.telegram.org/bots/api#getfile
    let url = format!(
        "https://api.telegram.org/file/bot{}/{}",
        bot_token, file_path
    );

    let r = bot.client.get(url).send().await?;

    if !r.status().is_success() {
        let text = r.text().await?;
        return Err(Error(text));
    }

    info!("Downloading file status: {}", r.status());

    Ok(r.bytes_stream())
}
