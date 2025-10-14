# tg-image-hosting

## deploy

edit wrangler.toml

```shell
copy wrangler.template.toml wrangler.toml
vim wrangler.toml
```

change `TELEGRAM_TOKEN`, `MAINTAINER_ID` to your bot token and id.  
change `database_name`, `database_id` to your d1 database name and uuid.

deploy

```shell
npx wrangler deploy
```

register webhook

```shell
curl https://<your-workers-domain>/tgbot/register
```

then send image/file to your telegram bot or channel(invite bot to channel as admin).

![screenshot](https://raw.githubusercontent.com/Asutorufa/tg-image-hosting/refs/heads/main/assets/images/image.png)
