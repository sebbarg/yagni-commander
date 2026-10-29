//! Markdown mode's images: which ones are shown (local image files, see
//! `local_image`), and the source rewritten so every other image becomes its
//! alt text, since the view draws a failed image as nothing. Images are found
//! with the parser the view uses (`markdown`, by source position): `![]()`,
//! reference images (rewritten inline, as the view has no references) and
//! HTML `<img>` tags.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};

use markdown::ParseOptions;
use markdown::mdast::Node;

/// Larger image files are not shown: an image is decoded to full size in
/// memory.
pub const IMAGE_CAP: u64 = 20 << 20;

/// What the view decodes (gpui's image formats, plus SVG).
const EXTENSIONS: [&str; 12] = [
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "tif", "tiff", "svg", "qoi", "tga",
];

/// The source to show, and the file of each image left in it, by the URL
/// the view will ask for.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Images {
    pub text: String,
    pub paths: HashMap<String, PathBuf>,
}

impl Images {
    /// Finds the images in `source`, a Markdown file in `dir`, and checks
    /// their files.
    pub fn prepare(source: &str, dir: &Path) -> Self {
        Self::prepare_with(source, |url| local_image(dir, url))
    }

    /// `find` says which file an image URL shows, if any.
    fn prepare_with(source: &str, find: impl Fn(&str) -> Option<PathBuf>) -> Self {
        let mut found = Found {
            definitions: HashMap::new(),
            edits: Vec::new(),
            shown: HashSet::new(),
            find: &find,
        };
        // Only MDX syntax can fail to parse; without a tree nothing changes.
        if let Ok(root) = markdown::to_mdast(source, &options()) {
            collect_definitions(&root, &mut found.definitions);
            found.walk(&root, false);
        }
        let mut edits = found.edits;
        edits.sort_by_key(|(range, _)| range.start);
        let mut text = String::with_capacity(source.len());
        let mut at = 0;
        for (range, replacement) in edits {
            if range.start < at || range.end > source.len() {
                continue; // overlapping or out of range: leave as is
            }
            text.push_str(&source[at..range.start]);
            text.push_str(&replacement);
            at = range.end;
        }
        text.push_str(&source[at..]);
        let paths = found
            .shown
            .into_iter()
            .filter_map(|url| find(&url).map(|path| (url, path)))
            .collect();
        Self { text, paths }
    }
}

/// The view's options: GFM with math (`gpui-base` `parse_options`).
fn options() -> ParseOptions {
    let mut options = ParseOptions::gfm();
    options.constructs.math_text = true;
    options.constructs.math_flow = true;
    options
}

/// The file an image URL shows: a path relative to `dir` (or absolute),
/// percent-decoded, without `?` and `#` parts, to a regular file of at most
/// `IMAGE_CAP` with an image extension. A URL with a scheme or a host is
/// never a file (nothing is fetched).
pub fn local_image(dir: &Path, url: &str) -> Option<PathBuf> {
    let url = url.trim();
    if url.is_empty() || url.starts_with("//") || has_scheme(url) {
        return None;
    }
    let path = url.split(['?', '#']).next()?;
    let path = percent_encoding::percent_decode_str(path)
        .decode_utf8()
        .ok()?;
    let path = dir.join(path.as_ref());
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    if !EXTENSIONS.contains(&extension.as_str()) {
        return None;
    }
    let meta = fs::metadata(&path).ok()?;
    (meta.is_file() && meta.len() <= IMAGE_CAP).then_some(path)
}

/// `scheme:` per RFC 3986, longer than one letter (`C:` is a drive).
pub fn has_scheme(url: &str) -> bool {
    let Some((scheme, _)) = url.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    scheme.len() > 1
        && chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

struct Found<'a, F: Fn(&str) -> Option<PathBuf>> {
    /// Normalized label to URL.
    definitions: HashMap<String, String>,
    /// Source ranges and what replaces them.
    edits: Vec<(Range<usize>, String)>,
    /// URLs of the images left in.
    shown: HashSet<String>,
    find: &'a F,
}

impl<F: Fn(&str) -> Option<PathBuf>> Found<'_, F> {
    /// `inline`: inside a paragraph, a heading or a cell (Markdown text),
    /// not in an HTML block.
    fn walk(&mut self, node: &Node, inline: bool) {
        let range = node.position().map(|p| p.start.offset..p.end.offset);
        match (node, range) {
            (Node::Image(image), Some(range)) => {
                if self.shows(&image.url) {
                    self.shown.insert(image.url.clone());
                } else {
                    self.edits.push((range, markdown_text(&image.alt)));
                }
            }
            (Node::ImageReference(image), Some(range)) => {
                let url = self.definitions.get(&normalize(&image.identifier)).cloned();
                let replacement = match url {
                    Some(url) if self.shows(&url) => {
                        let inline = format!("![{}](<{}>)", markdown_text(&image.alt), url);
                        self.shown.insert(url);
                        inline
                    }
                    _ => markdown_text(&image.alt),
                };
                self.edits.push((range, replacement));
            }
            (Node::Html(html), Some(range)) => self.html(&html.value, range.start, inline),
            _ => {
                let inline = inline
                    || matches!(
                        node,
                        Node::Paragraph(_) | Node::Heading(_) | Node::TableCell(_)
                    );
                for child in node.children().into_iter().flatten() {
                    self.walk(child, inline);
                }
            }
        }
    }

    /// The `<img>` tags in an HTML node starting at `offset`: the ones not
    /// shown become their alt text, escaped for where they stand.
    fn html(&mut self, value: &str, offset: usize, inline: bool) {
        for (tag, attributes) in img_tags(value) {
            let src = attribute(attributes, "src").unwrap_or_default();
            if self.shows(&src) {
                self.shown.insert(src);
                continue;
            }
            let alt = attribute(attributes, "alt").unwrap_or_default();
            let text = if inline {
                markdown_text(&alt)
            } else {
                html_text(&alt)
            };
            self.edits
                .push((offset + tag.start..offset + tag.end, text));
        }
    }

    fn shows(&self, url: &str) -> bool {
        (self.find)(url).is_some()
    }
}

fn collect_definitions(node: &Node, definitions: &mut HashMap<String, String>) {
    if let Node::Definition(definition) = node {
        // The first definition of a label wins (CommonMark).
        definitions
            .entry(normalize(&definition.identifier))
            .or_insert_with(|| definition.url.clone());
    }
    for child in node.children().into_iter().flatten() {
        collect_definitions(child, definitions);
    }
}

/// A label as CommonMark matches it: whitespace collapsed, case folded.
fn normalize(label: &str) -> String {
    label
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// `text` as literal Markdown: every ASCII punctuation escaped.
fn markdown_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_ascii_punctuation() {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// `text` as literal HTML.
fn html_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Each `<img ...>` tag in `html`: its range and its attribute text.
fn img_tags(html: &str) -> Vec<(Range<usize>, &str)> {
    let lower = html.to_ascii_lowercase();
    let mut tags = Vec::new();
    let mut from = 0;
    while let Some(i) = lower[from..].find("<img") {
        let start = from + i;
        let after = start + "<img".len();
        // `<imgx>` is another tag.
        if !lower[after..].starts_with(|c: char| c.is_ascii_whitespace() || c == '/' || c == '>') {
            from = after;
            continue;
        }
        let Some(end) = tag_end(html, after) else {
            break;
        };
        tags.push((start..end, html[after..end - 1].trim_end_matches('/')));
        from = end;
    }
    tags
}

/// Just past the `>` closing a tag whose attributes start at `from`
/// (a `>` inside quotes doesn't close it).
fn tag_end(html: &str, from: usize) -> Option<usize> {
    let mut quote = None;
    for (i, c) in html[from..].char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, '>') => return Some(from + i + 1),
            _ => {}
        }
    }
    None
}

/// The value of attribute `name` (case-insensitive; quoted or not), with the
/// common character references decoded.
fn attribute(attributes: &str, name: &str) -> Option<String> {
    let mut rest = attributes;
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_ascii_whitespace() || c == '/');
        if rest.is_empty() {
            return None;
        }
        let key_end = rest
            .find(|c: char| c.is_ascii_whitespace() || c == '=' || c == '/')
            .unwrap_or(rest.len());
        let key = &rest[..key_end];
        rest = rest[key_end..].trim_start();
        let value = if let Some(after) = rest.strip_prefix('=') {
            let after = after.trim_start();
            let (value, next) = match after.chars().next() {
                Some(q @ ('"' | '\'')) => {
                    let body = &after[1..];
                    let end = body.find(q).unwrap_or(body.len());
                    (&body[..end], body.get(end + 1..).unwrap_or(""))
                }
                _ => {
                    let end = after
                        .find(|c: char| c.is_ascii_whitespace())
                        .unwrap_or(after.len());
                    (&after[..end], &after[end..])
                }
            };
            rest = next;
            Some(value)
        } else {
            None
        };
        if key.eq_ignore_ascii_case(name) {
            return Some(decode_references(value.unwrap_or("")));
        }
    }
}

fn decode_references(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Images whose URL starts with `ok/` are shown, as `/files/<url>`.
    fn prepare(source: &str) -> Images {
        Images::prepare_with(source, |url| {
            url.starts_with("ok/")
                .then(|| PathBuf::from("/files").join(url))
        })
    }

    fn text(source: &str) -> String {
        prepare(source).text
    }

    #[test]
    fn shown_images_stay_and_are_mapped_to_their_files() {
        let images = prepare("A ![pic](ok/a.png) b.\n");
        assert_eq!(images.text, "A ![pic](ok/a.png) b.\n");
        assert_eq!(
            images.paths,
            HashMap::from([("ok/a.png".to_owned(), PathBuf::from("/files/ok/a.png"))])
        );
    }

    #[test]
    fn other_images_become_their_alt_text() {
        assert_eq!(
            text("A ![remote *badge*](https://x/b.svg) b.\n"),
            "A remote badge b.\n"
        );
        assert_eq!(text("![](missing.png)\n"), "\n");
        assert_eq!(text("![a.b](x)\n"), "a\\.b\n", "literal, not Markdown");
        // Inside a link the alt text keeps the link.
        assert_eq!(
            text("[![build](https://ci/x.svg)](https://ci)\n"),
            "[build](https://ci)\n"
        );
        assert!(prepare("![x](https://x/y.png)").paths.is_empty());
    }

    #[test]
    fn reference_images_are_written_inline_or_as_alt_text() {
        let source =
            "![one][a] and ![two][B] and ![three][none]\n\n[a]: ok/a.png\n[b]: https://x/b.png\n";
        let images = prepare(source);
        assert_eq!(
            images.text,
            "![one](<ok/a.png>) and two and ![three][none]\n\n[a]: ok/a.png\n[b]: https://x/b.png\n",
            "an undefined reference is no image (CommonMark): left as written"
        );
        assert_eq!(images.paths.len(), 1);
        // Collapsed and shortcut forms, labels matched loosely.
        assert_eq!(
            text("![Big  Pic][] ![big pic]\n\n[big pic]: ok/p.png\n"),
            "![Big  Pic](<ok/p.png>) ![big pic](<ok/p.png>)\n\n[big pic]: ok/p.png\n"
        );
    }

    #[test]
    fn html_img_tags_in_blocks_and_text() {
        let block = "<p align=\"center\">\n  <img src=\"ok/s.png\" alt=\"shot\"><br>\n  <IMG SRC='https://x/y.png' ALT=\"a <b> & c\" />\n</p>\n";
        let images = prepare(block);
        assert_eq!(
            images.text,
            "<p align=\"center\">\n  <img src=\"ok/s.png\" alt=\"shot\"><br>\n  a &lt;b&gt; &amp; c\n</p>\n"
        );
        assert!(images.paths.contains_key("ok/s.png"));
        assert_eq!(
            text("Inline <img src=\"nope.png\" alt=\"x_y\"> here.\n"),
            "Inline x\\_y here.\n"
        );
        assert_eq!(
            text("Tag <imgx src=\"a\"> stays.\n"),
            "Tag <imgx src=\"a\"> stays.\n"
        );
    }

    #[test]
    fn attributes_are_read_like_html() {
        let get = |attrs: &str, name: &str| attribute(attrs, name);
        assert_eq!(get(" src=a.png alt=x", "src").as_deref(), Some("a.png"));
        assert_eq!(get(" alt='it''s' src=b", "src").as_deref(), Some("b"));
        assert_eq!(
            get(" data-x src = \"c d.png\"", "SRC").as_deref(),
            Some("c d.png")
        );
        assert_eq!(get(" src=\"a&amp;b\"", "src").as_deref(), Some("a&b"));
        assert_eq!(get(" hidden", "hidden").as_deref(), Some(""));
        assert_eq!(get(" alt=\"x", "alt").as_deref(), Some("x"));
        assert_eq!(get(" alt=", "alt").as_deref(), Some(""));
        assert_eq!(get("", "src"), None);
        // A `>` in quotes doesn't end the tag.
        assert_eq!(img_tags("<img alt=\"a>b\" src=x>")[0].0, 0..21);
        assert!(img_tags("<img src=x").is_empty(), "unclosed");
    }

    #[test]
    fn code_is_left_alone() {
        let source = "`![x](https://a)` and\n\n```\n![y](https://b)\n<img src=c>\n```\n";
        assert_eq!(text(source), source);
    }

    #[test]
    fn local_images_are_regular_files_under_the_cap() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("doc");
        fs::create_dir_all(dir.join("my pics")).unwrap();
        fs::write(dir.join("a.PNG"), b"png").unwrap();
        fs::write(dir.join("my pics/b.svg"), b"<svg/>").unwrap();
        fs::write(tmp.path().join("up.gif"), b"gif").unwrap();
        fs::write(dir.join("notes.txt"), b"text").unwrap();
        fs::create_dir(dir.join("dir.png")).unwrap();
        let big = fs::File::create(dir.join("big.jpg")).unwrap();
        big.set_len(IMAGE_CAP + 1).unwrap();
        let cap = fs::File::create(dir.join("cap.jpg")).unwrap();
        cap.set_len(IMAGE_CAP).unwrap();

        let find = |url: &str| local_image(&dir, url);
        assert_eq!(find("a.PNG"), Some(dir.join("a.PNG")));
        assert_eq!(find(" a.PNG?raw=true#top "), Some(dir.join("a.PNG")));
        assert_eq!(find("my%20pics/b.svg"), Some(dir.join("my pics/b.svg")));
        assert_eq!(find("../up.gif"), Some(dir.join("../up.gif")));
        let absolute = tmp.path().join("up.gif");
        assert_eq!(find(absolute.to_str().unwrap()), Some(absolute.clone()));
        assert_eq!(find("cap.jpg"), Some(dir.join("cap.jpg")));
        assert_eq!(find("big.jpg"), None, "over the cap");
        assert_eq!(find("notes.txt"), None, "not an image type");
        assert_eq!(find("dir.png"), None, "a folder");
        assert_eq!(find("missing.png"), None);
        assert_eq!(find("noextension"), None);
        assert_eq!(find("%FF.png"), None, "not UTF-8");
        assert_eq!(find(""), None);
        assert_eq!(find("https://x/a.PNG"), None);
        assert_eq!(find("file:///etc/a.png"), None);
        assert_eq!(find("//host/a.png"), None);
        // `C:` is no scheme: a relative path here.
        assert_eq!(find("C:/a.png"), None);
    }

    #[test]
    fn has_scheme_follows_rfc_3986() {
        assert!(has_scheme("https://x"));
        assert!(has_scheme("git+ssh://x"));
        assert!(has_scheme("data:image/png;base64,AAAA"));
        assert!(!has_scheme("C:/x"));
        assert!(!has_scheme("1a:x"));
        assert!(!has_scheme("a b:x"));
        assert!(!has_scheme("x.png"));
    }
}
