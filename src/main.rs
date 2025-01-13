use std::{
    collections::HashMap,
    io::Read,
    path::{Path, PathBuf},
};

use bpaf::Bpaf;
use color_eyre::eyre::{bail, eyre, Result};
use dom_smoothie::{Article as ExtractArticle, Config as ExtractConfig, Readability};
use epub_builder::{EpubBuilder, EpubContent, ZipLibrary};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use url::Url;

const APP_NAME: &str = "stash";
const USER_AGENTS: &[&str] = &[
    "curl/8.11",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
];

#[derive(Deserialize, Debug)]
struct Config {
    output_dir: String,
}

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
    fn extract(&self, uri: &str, html: &str) -> Result<Article> {
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

fn auto_extract(url: &str, html: &str) -> Result<Article> {
    let cfg = ExtractConfig::default();
    let mut readability = Readability::new(html, Some(url), Some(cfg))?;
    let article: ExtractArticle = readability.parse()?;
    Ok(Article {
        url: url.to_string(),
        title: article.title,
        authors: article.byline.unwrap_or_default(),
        published_at: article.published_time.unwrap_or_default(),
        content: article.content.to_string(), // HTML content
    })
}

fn selector(sel: &str) -> Result<Selector> {
    Selector::parse(sel).map_err(|err| eyre!(err.to_string()))
}

fn manual_extract(
    url: &str,
    html: &str,
    title_sel: &str,
    body_sel: &str,
    authors_sel: &str,
    date_sel: &str,
) -> Result<Article> {
    let doc = Html::parse_document(html);
    let title_sel = selector(title_sel)?;
    let body_sel = selector(body_sel)?;
    let authors_sel = selector(authors_sel)?;
    let date_sel = selector(date_sel)?;

    let mut entry = Article {
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
        bail!("Could not find main content element.");
    }
    if entry.content.is_empty() {
        bail!("Main content element is empty.");
    }

    Ok(entry)
}

#[derive(Serialize, Default)]
struct Article {
    url: String,
    title: String,
    content: String,
    authors: String,
    published_at: String,
}
impl Article {
    fn build_epub(&self, output_dir: &Path) -> epub_builder::Result<PathBuf> {
        let fname = slug::slugify(&self.title);
        let fname = format!("{fname}.epub");
        let path = output_dir.join(fname);
        let output = fs_err::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&path)
            .unwrap();

        let content = EpubContent::new("main.xhtml", self.content.as_bytes())
            .title(&self.title)
            .reftype(epub_builder::ReferenceType::Text);
        let mut builder = EpubBuilder::new(ZipLibrary::new()?)?;

        builder
            .metadata("author", &self.authors)?
            .metadata("title", &self.title)?
            .metadata("description", &self.url)?
            .add_content(content)?;

        match dateparser::parse(&self.published_at) {
            Ok(parsed) => {
                builder.set_publication_date(parsed);
            }
            Err(err) => {
                eprintln!(
                    "Failed to parse published datetime {}: {}",
                    self.published_at, err
                );
            }
        }

        builder.generate(output)?;
        Ok(path)
    }
}

#[derive(Serialize, Deserialize, Default, Debug)]
#[serde(transparent)]
struct Extractor {
    configs: HashMap<String, ExtractionMethod>,
}
impl Extractor {
    fn load(path: &Path) -> Result<Self> {
        Ok(toml::from_str(&fs_err::read_to_string(path)?)?)
    }

    fn fetch_article(&self, url: &str) -> Result<Article> {
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

        Err(eyre!("All user-agents failed."))
    }
}

#[derive(Clone, Debug, Bpaf)]
#[bpaf(options, version)]
/// An web article extractor.
/// Uses automatic or manually-defined-rule extraction,
/// then generates an epub from the extracted content.
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

fn main() -> Result<()> {
    let opts = args().run();

    let config_dir = dirs::config_dir()
        .expect("Config dir exists")
        .join(APP_NAME);
    let config_path = config_dir.join("config.toml");
    let config: Config = toml::from_str(&fs_err::read_to_string(config_path)?)?;

    let extractor_path = config_dir.join("sites.toml");
    let extractor = Extractor::load(&extractor_path)?;
    let entry = extractor.fetch_article(&opts.url)?;

    // Preview results.
    println!("Title: {}", entry.title);
    println!("Authors: {}", entry.authors);
    println!("Published: {}", entry.published_at);
    println!("Content: {}", entry.content);
    if ask_confirm("Ok?") {
        let output_dir: PathBuf = shellexpand::tilde(&config.output_dir).to_string().into();
        let path = entry.build_epub(&output_dir)?;
        println!("{}", path.display());
    }
    Ok(())
}
