use std::{collections::HashMap, io::Read, path::Path};

use bpaf::Bpaf;
use dom_smoothie::{Article, Config, Readability};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use url::Url;

const APP_NAME: &str = "stash";
const USER_AGENTS: &[&str] = &[
    "curl/8.11",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
];

#[derive(Serialize, Deserialize, Default, Debug)]
#[serde(untagged)]
enum ExtractionMethod {
    #[default]
    Auto,
    Manual {
        title: String,
        body: String,
        authors: String,
        date: String,
    },
}
impl ExtractionMethod {
    fn extract(&self, uri: &str, html: &str) -> anyhow::Result<WallabagEntry> {
        match self {
            Self::Auto => auto_extract(uri, html),
            Self::Manual {
                title,
                body,
                authors,
                date,
            } => manual_extract(uri, html, title, body, authors, date),
        }
    }
}

fn auto_extract(url: &str, html: &str) -> anyhow::Result<WallabagEntry> {
    let cfg = Config::default();
    let mut readability = Readability::new(html, Some(url), Some(cfg))?;
    let article: Article = readability.parse()?;
    Ok(WallabagEntry {
        url: url.to_string(),
        title: article.title,
        authors: article.byline.unwrap_or_default(),
        published_at: article.published_time.unwrap_or_default(),
        content: article.content.to_string(), // HTML content
    })
}

fn selector(sel: &str) -> anyhow::Result<Selector> {
    Selector::parse(sel).map_err(|err| anyhow::anyhow!(err.to_string()))
}

fn manual_extract(
    url: &str,
    html: &str,
    title_sel: &str,
    body_sel: &str,
    authors_sel: &str,
    date_sel: &str,
) -> anyhow::Result<WallabagEntry> {
    let doc = Html::parse_document(html);
    let title_sel = selector(title_sel)?;
    let body_sel = selector(body_sel)?;
    let authors_sel = selector(authors_sel)?;
    let date_sel = selector(date_sel)?;

    let mut entry = WallabagEntry {
        url: url.to_string(),
        ..Default::default()
    };

    if let Some(el) = doc.select(&title_sel).next() {
        entry.title = el.text().collect::<Vec<_>>().join("");
    } else {
        eprintln!("WARN: Title element not found.");
    }

    if let Some(el) = doc.select(&authors_sel).next() {
        entry.authors = el.text().collect::<Vec<_>>().join("");
    } else {
        eprintln!("WARN: Authors element not found.");
    }

    if let Some(el) = doc.select(&date_sel).next() {
        entry.published_at = el.text().collect::<Vec<_>>().join("");
    } else {
        eprintln!("WARN: Published At element not found.");
    }

    if let Some(el) = doc.select(&body_sel).next() {
        entry.content = el.inner_html();
    } else {
        anyhow::bail!("Could not find main content element.");
    }
    if entry.content.is_empty() {
        anyhow::bail!("Main content element is empty.");
    }

    Ok(entry)
}

#[derive(Serialize, Default)]
struct WallabagEntry {
    url: String,
    title: String,
    content: String,
    authors: String,
    published_at: String,
}

struct WallabagClient {
    access_token: String,
}
impl WallabagClient {
    fn new(access_token: String) -> Self {
        Self { access_token }
    }

    // <https://app.wallabag.it/api/doc/>
    fn send_entry(&self, entry: &WallabagEntry) -> Result<ureq::Response, ureq::Error> {
        let token = format!("Bearer {}", self.access_token);
        ureq::post("https://app.wallabag.it/api/entries")
            .set("Authorization", &token)
            .send_json(entry)
    }
}

#[derive(Deserialize)]
struct AuthResponse {
    access_token: String,
}

#[derive(Deserialize)]
struct Credentials {
    client_id: String,
    client_secret: String,
    username: String,
    password: String,
}
impl Credentials {
    fn authenticate(&self) -> anyhow::Result<String> {
        let resp = ureq::post("https://app.wallabag.it/oauth/v2/token")
            .send_json(ureq::json!({
                "grant_type": "password",
                "client_id": &self.client_id,
                "client_secret": &self.client_secret,
                "username": &self.username,
                "password": &self.password,
            }))
            .inspect_err(|err| match err {
                ureq::Error::Status(code, resp) => {
                    eprintln!("[{code}]: {:?}", resp.status_text())
                }
                _ => (),
            })?
            .into_json::<AuthResponse>()?;
        Ok(resp.access_token)
    }
}

#[derive(Serialize, Deserialize, Default, Debug)]
#[serde(transparent)]
struct Extractor {
    configs: HashMap<String, ExtractionMethod>,
}
impl Extractor {
    fn load(path: &Path) -> anyhow::Result<Self> {
        Ok(toml::from_str(&fs_err::read_to_string(path)?)?)
    }

    fn fetch_article(&self, url: &str) -> anyhow::Result<WallabagEntry> {
        let url_parsed = Url::parse(url)?;
        let default = ExtractionMethod::default();
        let method = match url_parsed.domain() {
            Some(domain) => {
                println!("Domain: {}", domain);
                self.configs.get(domain).unwrap_or(&default)
            }
            None => &default,
        };

        for ua in USER_AGENTS {
            let resp = ureq::get(url).set("User-Agent", ua).call();
            let html = match resp {
                Err(err) => match err {
                    ureq::Error::Status(code, resp) => {
                        let err = format!("[{ua}]: {code} {:?}", resp.status_text());
                        let body = resp.into_string()?;
                        let log_path = dirs::cache_dir()
                            .expect("Cache dir present")
                            .join("stash-error.log");
                        eprintln!(
                            "{}\nResponse content written to `{}`.",
                            err,
                            log_path.display()
                        );
                        fs_err::write(log_path, body).expect("Unable to write file");
                        continue;
                    }
                    err => {
                        eprintln!("[{ua}]: {err}");
                        continue;
                    }
                },
                Ok(resp) => resp.into_string(),
            }?;
            return method.extract(url, &html);
        }

        Err(anyhow::anyhow!("All user-agents failed."))
    }
}

#[derive(Clone, Debug, Bpaf)]
#[bpaf(options, version)]
/// An extractor for Wallabag.
/// Uses automatic or manually-defined-rule extraction,
/// then sends the extracted content to Wallabag.
struct Args {
    /// Url to extract.
    #[bpaf(positional("URL"))]
    url: String,
}

fn ask_confirm(question: &str) -> bool {
    println!("{}", question);
    let mut input = [0];
    let _ = std::io::stdin().read(&mut input);
    match input[0] as char {
        'y' | 'Y' => true,
        _ => false,
    }
}

fn main() -> anyhow::Result<()> {
    let opts = args().run();

    let config_dir = dirs::config_dir()
        .expect("Config dir exists")
        .join(APP_NAME);
    let creds_path = config_dir.join("credentials.toml");
    let creds: Credentials = toml::from_str(&fs_err::read_to_string(creds_path)?)?;

    let extractor_path = config_dir.join("extractor.toml");
    let extractor = Extractor::load(&extractor_path)?;

    let token = creds.authenticate()?;
    let client = WallabagClient::new(token);
    let entry = extractor.fetch_article(&opts.url)?;

    // Preview results.
    println!("Title: {}", entry.title);
    println!("Authors: {}", entry.authors);
    println!("Published: {}", entry.published_at);
    println!("Content: {}", entry.content);
    if ask_confirm("Send to Wallabag?") {
        client.send_entry(&entry)?;
    }
    Ok(())
}
