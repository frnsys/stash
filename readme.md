# stash

This is an external extractor for Wallabag.

For automatic extraction it uses [`dom_smoothie`](https://github.com/niklak/dom_smoothie) which seems to do better than the `php-readability` used by Wallabag, and for manual extraction you can define CSS selectors for each field in `~/.config/stash/extractor.toml`:

```toml
["somedomain.com"]
title = ".content h2"
body = ".content .main"
authors = ".content .bylines"
date = ".content .published_at"
```

You need to include Wallabag app/API client credentials in `~/.config/stash/credentials.toml`:

```toml
client_id = "<client id>"
client_secret = "<client secret>"
username = "<username>"
password = "<password>"
```

Then to use:

```bash
stash <url>
```
