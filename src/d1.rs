use frankenstein::types::{Document, Message, PhotoSize, Video};
use serde::{Deserialize, Serialize};
use std::{ops::Deref, sync::Arc};
use wasm_bindgen::JsValue;
use worker::{D1Database, D1PreparedStatement};

use crate::error::Error;

pub static CREATE_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS [files](
    "file_unique_id" TEXT PRIMARY KEY,
    "file_id" TEXT UNIQUE,
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
)
;
"#;

pub static INSERT_FILE: &str = r#"
INSERT INTO files(
  file_id, file_unique_id, thumbnail_file_id, 
  thumbnail_file_unique_id, message_id, 
  user_id, file_name, file_size, mime_type, 
  add_time, update_time, file_path
) 
VALUES 
  (
    ?, 
    ?, 
    ?, 
    ?, 
    ?, 
    ?, 
    ?, 
    ?, 
    ?, 
    strftime('%s', 'now'), 
    strftime('%s', 'now'), 
    ?
  ) ON CONFLICT(file_unique_id) DO 
UPDATE 
SET 
  thumbnail_file_id = excluded.thumbnail_file_id, 
  thumbnail_file_unique_id = excluded.thumbnail_file_unique_id, 
  message_id = excluded.message_id, 
  user_id = excluded.user_id, 
  file_name = excluded.file_name, 
  file_size = excluded.file_size, 
  mime_type = excluded.mime_type, 
  update_time = strftime('%s', 'now'), 
  file_path = excluded.file_path
"#;

pub static SAVE_FILE_PATH: &str = r#"
UPDATE
    files
SET
    file_path = ?
WHERE
    file_unique_id = ?
"#;

pub static SELECT_FILE: &str = r#"
SELECT
    *
FROM
    files
WHERE
    file_id = ?
OR  file_unique_id = ?
"#;

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
            files.push(
                File::from(doc.deref())
                    .with_message_id(msg_id)
                    .with_user_id(user_id)
                    .with_file_path(get_file_path(doc.file_id).await?),
            );
        }
        if let Some(photos) = msg.photo {
            match photos.last() {
                Some(photo) => {
                    files.push(
                        File::from(photo)
                            .with_message_id(msg_id)
                            .with_user_id(user_id)
                            .with_file_path(get_file_path(photo.file_id.clone()).await?),
                    );
                }
                _ => {}
            };
        }
        if let Some(video) = msg.video {
            files.push(
                File::from(video.deref())
                    .with_message_id(msg_id)
                    .with_user_id(user_id)
                    .with_file_path(get_file_path(video.file_id).await?),
            );
        }
        Ok(files)
    }
}

impl From<&Video> for File {
    fn from(v: &Video) -> Self {
        let (thumbnail_file_id, thumbnail_file_unique_id) = match &v.thumbnail {
            Some(t) => (&t.file_id, &t.file_unique_id),
            None => (&String::new(), &String::new()),
        };
        File {
            file_id: v.file_id.clone(),
            file_unique_id: v.file_unique_id.clone(),
            thumbnail_file_id: thumbnail_file_id.clone(),
            thumbnail_file_unique_id: thumbnail_file_unique_id.clone(),
            file_size: v.file_size.unwrap_or_default(),
            mime_type: v.mime_type.clone().unwrap_or_default(),
            file_name: v.file_name.clone().unwrap_or_default(),
            add_time: 0,
            update_time: 0,
            message_id: 0,
            user_id: 0,
            file_path: "".to_string(),
        }
    }
}

impl From<&Document> for File {
    fn from(value: &Document) -> Self {
        let (thumbnail_file_id, thumbnail_file_unique_id) = match &value.thumbnail {
            Some(t) => (&t.file_id, &t.file_unique_id),
            None => (&String::new(), &String::new()),
        };
        File {
            file_id: value.file_id.clone(),
            file_unique_id: value.file_unique_id.clone(),
            thumbnail_file_id: thumbnail_file_id.clone(),
            thumbnail_file_unique_id: thumbnail_file_unique_id.clone(),
            file_size: value.file_size.unwrap_or_default(),
            mime_type: value.mime_type.clone().unwrap_or_default(),
            file_name: value.file_name.clone().unwrap_or_default(),
            add_time: 0,
            update_time: 0,
            message_id: 0,
            user_id: 0,
            file_path: "".to_string(),
        }
    }
}

impl From<&PhotoSize> for File {
    fn from(value: &PhotoSize) -> Self {
        File {
            file_id: value.file_id.clone(),
            file_unique_id: value.file_unique_id.clone(),
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

    pub async fn save_file_path(
        &self,
        file_unique_id: &String,
        file_path: &String,
    ) -> Result<(), Error> {
        self.db
            .prepare(SAVE_FILE_PATH)
            .bind(&vec![file_path.into(), file_unique_id.into()])?
            .run()
            .await?;
        Ok(())
    }

    fn save_statements(&self, files: &Vec<File>) -> Result<Vec<D1PreparedStatement>, Error> {
        let statement = self.db.prepare(INSERT_FILE);

        let mut statements = vec![];

        for f in files {
            let values: Vec<JsValue> = vec![
                (&f.file_id).into(),
                (&f.file_unique_id).into(),
                (&f.thumbnail_file_id).into(),
                (&f.thumbnail_file_unique_id).into(),
                f.message_id.into(),
                f.user_id.into(),
                (&f.file_name).into(),
                f.file_size.into(),
                (&f.mime_type).into(),
                (&f.file_path).into(),
            ];

            statements.push(statement.clone().bind(&values)?);
        }

        Ok(statements)
    }

    pub async fn save(&self, files: &Vec<File>) -> Result<(), Error> {
        if files.is_empty() {
            return Ok(());
        }
        match self.db.batch(self.save_statements(files)?).await {
            Ok(_) => Ok(()),
            Err(worker::Error::D1(e)) if e.cause().contains("no such table") => {
                self.init().await?;
                self.db.batch(self.save_statements(files)?).await?;
                Ok(())
            }
            Err(e) => Err(Error(e.to_string())),
        }
    }

    pub async fn get(&self, file_id: &String) -> Result<File, Error> {
        self.db
            .prepare(SELECT_FILE)
            .bind(&vec![file_id.into(), file_id.into()])?
            .first::<File>(None)
            .await?
            .ok_or(Error("File not found".to_string()))
    }
}
