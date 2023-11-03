use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use libs::ammonia;
use libs::elasticlunr::{lang, Index, IndexBuilder};
use libs::once_cell::sync::Lazy;

use config::{Config, Search};
use content::{Library, Section};
use errors::{bail, Result};
use libs::ahash::AHashMap;

pub const ELASTICLUNR_JS: &str = include_str!("elasticlunr.min.js");

static AMMONIA: Lazy<ammonia::Builder<'static>> = Lazy::new(|| {
    let mut clean_content = HashSet::new();
    clean_content.insert("script");
    clean_content.insert("style");
    let mut rm_tags = HashSet::new();
    rm_tags.insert("pre");
    let mut builder = ammonia::Builder::new();
    builder
        .tags(HashSet::new())
        .rm_tags(rm_tags)
        .tag_attributes(HashMap::new())
        .generic_attributes(HashSet::new())
        .link_rel(None)
        .allowed_classes(HashMap::new())
        .clean_content_tags(clean_content);
    builder
});

fn build_fields(search_config: &Search, mut index: IndexBuilder) -> IndexBuilder {
    if search_config.include_title {
        index = index.add_field("title");
    }

    if search_config.include_description {
        index = index.add_field("description");
    }

    if search_config.include_path {
        index = index.add_field_with_tokenizer("path", Box::new(path_tokenizer));
    }

    if search_config.include_content {
        index = index.add_field("body");
    }

    if search_config.include_tags {
        index = index.add_field("tags");
    }

    if search_config.include_categories {
        index = index.add_field("categories");
    }

    index
}

fn path_tokenizer(text: &str) -> Vec<String> {
    text.split(|c: char| c.is_whitespace() || c == '-' || c == '/')
        .filter(|s| !s.is_empty())
        .map(|s| s.trim().to_lowercase())
        .collect()
}

fn fill_index(
    search_config: &Search,
    title: &Option<String>,
    description: &Option<String>,
    path: &str,
    content: &str,
    categories: Vec<String>,
    tags: Vec<String>,
) -> Vec<String> {
    let mut row = vec![];

    if search_config.include_title {
        row.push(title.clone().unwrap_or_default());
    }

    if search_config.include_description {
        row.push(description.clone().unwrap_or_default());
    }

    if search_config.include_path {
        row.push(path.to_string());
    }

    if search_config.include_content {
        let body = AMMONIA.clean(content).to_string();
        if let Some(truncate_len) = search_config.truncate_content_length {
            // Not great for unicode
            // TODO: fix it like the truncate in Tera
            match body.char_indices().nth(truncate_len) {
                None => row.push(body),
                Some((idx, _)) => row.push((body[..idx]).to_string()),
            };
        } else {
            row.push(body);
        };
    }

    if search_config.include_tags {
        let tags = tags.join(" ");
        row.push(tags);
    }

    if search_config.include_categories {
        let categories = categories.join(" ");
        row.push(categories);
    }

    row
}

/// Returns the generated JSON index with all the documents of the site added using
/// the language given
/// Errors if the language given is not available in Elasticlunr
/// TODO: is making `in_search_index` apply to subsections of a `false` section useful?
pub fn build_index(lang: &str, library: &Library, config: &Config) -> Result<String> {
    let language = match lang::from_code(lang) {
        Some(l) => l,
        None => {
            bail!("Tried to build search index for language {} which is not supported", lang);
        }
    };
    let language_options = &config.languages[lang];
    let mut index = IndexBuilder::with_language(language);
    index = build_fields(&language_options.search, index);
    let mut index = index.build();

    for (_, section) in &library.sections {
        if section.lang == lang {
            add_section_to_index(&mut index, section, library, &language_options.search, lang);
        }
    }

    Ok(index.to_json())
}

fn add_section_to_index(
    index: &mut Index,
    section: &Section,
    library: &Library,
    search_config: &Search,
    lang: &str,
) {
    if !section.meta.in_search_index {
        return;
    }

    // Don't index redirecting sections
    if section.meta.redirect_to.is_none() {
        index.add_doc(
            &section.permalink,
            &fill_index(
                search_config,
                &section.meta.title,
                &section.meta.description,
                &section.path,
                &section.content,
                vec![],
                vec![],
            ),
        );
    }

    for key in &section.pages {
        let page = &library.pages[key];

        let (categories, tags) = get_categories_and_tags(&library, &page.permalink, lang);

        if !page.meta.in_search_index {
            continue;
        }

        index.add_doc(
            &page.permalink,
            &fill_index(
                search_config,
                &page.meta.title,
                &page.meta.description,
                &page.path,
                &page.content,
                categories,
                tags,
            ),
        );
    }
}

fn get_categories_and_tags(
    library: &Library,
    permalink: &str,
    lang: &str,
) -> (Vec<String>, Vec<String>) {
    let mut categories = vec![];
    let mut tags = vec![];
    let segments: Vec<&str> = permalink.trim_end_matches('/').rsplit('/').collect();
    if let Some(url_segment) = segments.get(0).copied() {
        if let Some(taxonomies) = library.taxonomies_def.get(lang) {
            if let Some(category_taxonomies) = taxonomies.get("categories") {
                match_file_stem_to_url_segment(&mut categories, url_segment, category_taxonomies);
            }
            if let Some(tag_taxonomies) = taxonomies.get("tags") {
                match_file_stem_to_url_segment(&mut tags, url_segment, tag_taxonomies);
            }
        }
    }
    (categories, tags)
}

fn match_file_stem_to_url_segment(output: &mut Vec<String>, url_segment: &str, taxonomies: &AHashMap<String, Vec<PathBuf>>) {
    for (taxonomy, file_paths) in taxonomies.iter() {
        for path_buffer in file_paths.iter() {
            if let Some(stem) = path_buffer.as_path().file_stem().map(|stem| stem.to_string_lossy().to_string()) {
                if url_segment == stem.as_str() {
                    output.push(taxonomy.to_string());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use config::Config;

    #[test]
    fn can_build_fields() {
        let mut config = Config::default();
        let index = build_fields(&config.search, IndexBuilder::new()).build();
        assert_eq!(index.get_fields(), vec!["title", "body"]);

        config.search.include_content = false;
        config.search.include_description = true;
        let index = build_fields(&config.search, IndexBuilder::new()).build();
        assert_eq!(index.get_fields(), vec!["title", "description"]);

        config.search.include_content = true;
        let index = build_fields(&config.search, IndexBuilder::new()).build();
        assert_eq!(index.get_fields(), vec!["title", "description", "body"]);

        config.search.include_title = false;
        let index = build_fields(&config.search, IndexBuilder::new()).build();
        assert_eq!(index.get_fields(), vec!["description", "body"]);
    }

    #[test]
    fn can_fill_index_default() {
        let config = Config::default();
        let title = Some("A title".to_string());
        let description = Some("A description".to_string());
        let path = "/a/page/".to_string();
        let content = "Some content".to_string();

        let res = fill_index(&config.search, &title, &description, &path, &content, vec![], vec![]);
        assert_eq!(res.len(), 2);
        assert_eq!(res[0], title.unwrap());
        assert_eq!(res[1], content);
    }

    #[test]
    fn can_fill_index_description() {
        let mut config = Config::default();
        config.search.include_description = true;
        let title = Some("A title".to_string());
        let description = Some("A description".to_string());
        let path = "/a/page/".to_string();
        let content = "Some content".to_string();

        let res = fill_index(&config.search, &title, &description, &path, &content, vec![], vec![]);
        assert_eq!(res.len(), 3);
        assert_eq!(res[0], title.unwrap());
        assert_eq!(res[1], description.unwrap());
        assert_eq!(res[2], content);
    }

    #[test]
    fn can_fill_index_truncated_content() {
        let mut config = Config::default();
        config.search.truncate_content_length = Some(5);
        let title = Some("A title".to_string());
        let description = Some("A description".to_string());
        let path = "/a/page/".to_string();
        let content = "Some content".to_string();

        let res = fill_index(&config.search, &title, &description, &path, &content, vec![], vec![]);
        assert_eq!(res.len(), 2);
        assert_eq!(res[0], title.unwrap());
        assert_eq!(res[1], content[..5]);
    }
}
