# stash

This is a web article extractor that outputs `epub`s.

For automatic extraction it uses [`dom_smoothie`](https://github.com/niklak/dom_smoothie). For manual extraction you can define CSS selectors for each field in `~/.config/stash/sites.toml`:

```toml
["somedomain.com"]
title = ".content h2"
body = ".content .main"
authors = ".content .bylines"
date = ".content .published_at"
```

You also need to create `~/.config/stash/config.toml` and define the output directory:

```toml
output_dir = "~/docs/articles"
```

Then to use:

```bash
stash <url>
```
