use frankenstein::AsyncTelegramApi;
use frankenstein::client_reqwest::Bot;
use frankenstein::methods::{GetFileParams, SendMessageParams, SetWebhookParams};
use frankenstein::types::{ChatId, LinkPreviewOptions, ReplyParameters};
use frankenstein::updates::UpdateContent;
use log::info;

use crate::d1::{D1, File};
use crate::error::Error;

#[derive(Clone)]
pub struct TgBot {
    pub bot: Bot,
    pub d1: D1,
    pub matainer: i64,
    pub bot_token: String,
}

impl TgBot {
    pub fn new(d1: D1, matainer: i64, bot_token: String) -> TgBot {
        TgBot {
            bot: Bot::new(&bot_token),
            d1,
            matainer,
            bot_token,
        }
    }

    pub fn matainer_id(&self) -> i64 {
        self.matainer
    }

    pub async fn set_webhook(self, url: &str) -> Result<(), Error> {
        info!("Registering webhook: {}", url);

        self.bot
            .set_webhook(&SetWebhookParams::builder().url(url).build())
            .await?;

        self.bot
            .send_message(
                &SendMessageParams::builder()
                    .chat_id(ChatId::Integer(self.matainer))
                    .text(format!("register webhook to {} successful", url))
                    .link_preview_options(LinkPreviewOptions::DISABLED)
                    .build(),
            )
            .await?;

        Ok(())
    }

    pub async fn handle(
        self,
        host: &String,
        update: frankenstein::updates::Update,
    ) -> Result<(), Error> {
        match update.content {
            UpdateContent::Message(msg)
            | UpdateContent::EditedMessage(msg)
            | UpdateContent::ChannelPost(msg) => {
                let chat_id = msg.chat.id;
                let msg_id = msg.message_id;

                let files = File::from_message(msg, async |f| {
                    let ff = self.bot.get_file(&GetFileParams { file_id: f }).await?;
                    match ff.result.file_path {
                        Some(p) => Ok(p),
                        None => return Err(Error("File path not found".to_string())),
                    }
                })
                .await?;

                if files.is_empty() {
                    return Ok(());
                }

                let mut response = String::new();

                match self.d1.save(files.clone()).await {
                    Ok(_) => {
                        for f in files {
                            let ext = match f.file_path.split('.').last() {
                                Some(ext) => format!(".{}", ext),
                                None => "".to_string(),
                            };
                            response +=
                                format!("https://{}/f/{}{}\n", host, f.file_id, ext).as_str();
                            response += format!("https://{}/f/{}{}\n", host, f.file_unique_id, ext)
                                .as_str();
                        }
                    }
                    Err(e) => {
                        response = format!("Error: {}", e);
                    }
                }

                self.bot
                    .send_message(
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

    pub async fn get_file_url(self, file_id: impl Into<String>) -> Result<String, Error> {
        let file_id = file_id.into();

        if file_id.is_empty() {
            return Err(Error("File id is empty".to_string()));
        }

        let file = self.d1.get(file_id).await?;

        let file_path = match file.file_path.as_str() {
            "" => {
                let f = self
                    .bot
                    .get_file(&GetFileParams {
                        file_id: file.file_id.clone(),
                    })
                    .await?;

                match f.result.file_path {
                    Some(p) => {
                        self.d1
                            .save_file_path(file.file_unique_id, p.clone())
                            .await?;
                        p
                    }
                    None => return Err(Error("File path not found".to_string())),
                }
            }
            _ => file.file_path,
        };

        info!("File path: {}", file_path);

        // https://core.telegram.org/bots/api#getfile
        let url = format!(
            "https://api.telegram.org/file/bot{}/{}",
            self.bot_token, file_path
        );

        Ok(url)
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
