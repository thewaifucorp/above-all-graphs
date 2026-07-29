//! Native text and metadata extraction for binary documents — P1.9 of
//! `docs/capability-coverage.md`.
//!
//! A PDF, a `.docx`, a spreadsheet, or a screenshot used to enter the graph as
//! a `Doc` node with no description, waiting for a host agent to look at it.
//! That works, and it costs a vision pass per file and only happens for files
//! an agent happens to open. Most of these formats carry their text in the
//! open: a `.docx` is a zip of XML, a spreadsheet is a table of strings, a PDF
//! has a text layer unless it is a scan.
//!
//! So text is extracted here first, and the host-agent path in `crate::docs`
//! stays exactly where it was — [`describe`](crate::docs::format) still
//! overwrites what this produced, because a description of what a diagram
//! *shows* beats the words that happen to be printed on it.
//!
//! Nothing here is OCR and nothing here is speech recognition. An image
//! contributes its dimensions and camera/authoring metadata; a video
//! contributes the transcript sitting next to it as a `.srt`/`.vtt` sidecar, if
//! one is there, and nothing otherwise.

use std::io::Read as _;
use std::path::Path;

/// Longest extracted text kept per document. A doc's text is stored in the
/// node description, read into agent context, and matched against symbol
/// names; a 400-page PDF pasted whole would crowd out everything else.
const MAX_TEXT: usize = 40_000;

/// Extensions this module can read without a vision pass.
#[must_use]
pub fn is_extractable(path: &str) -> bool {
    matches!(
        extension(path).as_str(),
        "pdf"
            | "docx"
            | "pptx"
            | "odt"
            | "odp"
            | "xlsx"
            | "xlsm"
            | "xls"
            | "ods"
            | "csv"
            | "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "webp"
            | "svg"
            | "mp4"
            | "mov"
            | "avi"
            | "mkv"
            | "webm"
    )
}

fn extension(path: &str) -> String {
    Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// Reads what a document says, or `None` when nothing can be read from it.
///
/// `None` is not failure — it is the honest answer for a scanned PDF, an
/// unlabelled screenshot, or a video with no transcript beside it, and it
/// leaves the node exactly as the host-agent description path expects to find
/// it.
#[must_use]
pub fn text(path: &Path) -> Option<String> {
    let name = path.to_string_lossy().to_string();
    let extracted = match extension(&name).as_str() {
        "pdf" => from_pdf(path),
        "docx" => from_ooxml(path, &["word/document.xml"], "w:p"),
        "pptx" => from_ooxml_glob(path, "ppt/slides/slide", "a:p"),
        "odt" | "odp" => from_ooxml(path, &["content.xml"], "text:p"),
        "xlsx" | "xlsm" | "xls" | "ods" => from_spreadsheet(path),
        "csv" => from_csv(path),
        "svg" => from_svg(path),
        "png" | "jpg" | "jpeg" | "gif" | "webp" => from_image(path),
        "mp4" | "mov" | "avi" | "mkv" | "webm" => from_sidecar_transcript(path),
        _ => None,
    }?;
    let trimmed = collapse(&extracted);
    (!trimmed.is_empty()).then(|| truncate(&trimmed))
}

/// Squeezes runs of whitespace, because every one of these formats produces
/// them and a description full of blank lines is a description nobody reads.
fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate(text: &str) -> String {
    if text.chars().count() <= MAX_TEXT {
        return text.to_string();
    }
    let kept: String = text.chars().take(MAX_TEXT).collect();
    format!("{kept} […text truncated at {MAX_TEXT} characters]")
}

/// A PDF's text layer. A scan has none, and no amount of trying changes that.
fn from_pdf(path: &Path) -> Option<String> {
    // `pdf-extract` panics on some malformed files rather than erroring, and
    // one bad PDF must not take down an index pass.
    let path = path.to_path_buf();
    let extracted =
        std::panic::catch_unwind(move || pdf_extract::extract_text(&path).ok()).ok()??;
    Some(extracted)
}

/// Text of one or more XML parts inside a zip container.
///
/// `paragraph` is the element that ends a block, so a document does not come
/// back as one run-on line: `w:p` in `.docx`, `a:p` in `.pptx`, `text:p` in
/// `OpenDocument`.
fn from_ooxml(path: &Path, parts: &[&str], paragraph: &str) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    let mut out = String::new();
    for part in parts {
        let Ok(mut entry) = archive.by_name(part) else {
            continue;
        };
        let mut xml = String::new();
        if entry.read_to_string(&mut xml).is_ok() {
            out.push_str(&xml_text(&xml, paragraph));
            out.push('\n');
        }
    }
    Some(out)
}

/// Same, for parts whose names are numbered (`ppt/slides/slide1.xml`, …).
fn from_ooxml_glob(path: &Path, prefix: &str, paragraph: &str) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    let mut names: Vec<String> = archive
        .file_names()
        .filter(|name| name.starts_with(prefix) && name.to_ascii_lowercase().ends_with(".xml"))
        .map(str::to_string)
        .collect();
    names.sort();
    let mut out = String::new();
    for name in names {
        let Ok(mut entry) = archive.by_name(&name) else {
            continue;
        };
        let mut xml = String::new();
        if entry.read_to_string(&mut xml).is_ok() {
            out.push_str(&xml_text(&xml, paragraph));
            out.push('\n');
        }
    }
    Some(out)
}

/// Character data of an XML document, with a newline where each `paragraph`
/// element closes.
fn xml_text(xml: &str, paragraph: &str) -> String {
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut out = String::new();
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(quick_xml::events::Event::Text(text)) => {
                if let Ok(value) = text.decode() {
                    out.push_str(value.as_ref());
                }
            }
            Ok(quick_xml::events::Event::End(end)) => {
                if end.name().as_ref() == paragraph.as_bytes() {
                    out.push('\n');
                }
            }
            Ok(quick_xml::events::Event::Eof) | Err(_) => break,
            Ok(_) => {}
        }
        buffer.clear();
    }
    out
}

/// A workbook, sheet by sheet: the sheet name, then its non-empty cells.
///
/// Cells rather than a rendered grid, because what a spreadsheet contributes
/// to a code graph is its words — column headers, sheet names, the identifier
/// someone typed into a config tab.
fn from_spreadsheet(path: &Path) -> Option<String> {
    let mut workbook = calamine::open_workbook_auto(path).ok()?;
    let mut out = String::new();
    for name in calamine::Reader::sheet_names(&workbook).clone() {
        let Ok(range) = calamine::Reader::worksheet_range(&mut workbook, &name) else {
            continue;
        };
        out.push_str(&name);
        out.push('\n');
        for row in range.rows() {
            let line = row
                .iter()
                .map(std::string::ToString::to_string)
                .filter(|cell| !cell.is_empty())
                .collect::<Vec<_>>()
                .join(" | ");
            if !line.is_empty() {
                out.push_str(&line);
                out.push('\n');
            }
        }
    }
    Some(out)
}

fn from_csv(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// An SVG is XML, so its labels are readable — which is most of what a diagram
/// contributes.
fn from_svg(path: &Path) -> Option<String> {
    let xml = std::fs::read_to_string(path).ok()?;
    Some(xml_text(&xml, "text"))
}

/// What an image says about itself: its size, and the metadata its authoring
/// tool wrote. Not what it depicts — that is what a vision pass is for, and
/// `aag describe` still overwrites this.
fn from_image(path: &Path) -> Option<String> {
    let mut parts = Vec::new();
    if let Ok(size) = imagesize::size(path) {
        parts.push(format!("{}×{} pixels", size.width, size.height));
    }
    if let Ok(file) = std::fs::File::open(path) {
        let mut reader = std::io::BufReader::new(file);
        if let Ok(exif) = exif::Reader::new().read_from_container(&mut reader) {
            for field in exif.fields() {
                if !INTERESTING_EXIF.contains(&field.tag.to_string().as_str()) {
                    continue;
                }
                let value = field.display_value().with_unit(&exif).to_string();
                let value = value.trim_matches('"');
                if !value.is_empty() {
                    parts.push(format!("{}: {value}", field.tag));
                }
            }
        }
    }
    (!parts.is_empty()).then(|| parts.join("; "))
}

/// EXIF tags worth carrying into a code graph: what made the image, when, and
/// anything a human typed into it. Camera exposure settings are not that.
const INTERESTING_EXIF: &[&str] = &[
    "Software",
    "Make",
    "Model",
    "DateTime",
    "DateTimeOriginal",
    "ImageDescription",
    "Artist",
    "Copyright",
    "UserComment",
    "XPTitle",
    "XPComment",
    "XPSubject",
];

/// A media file's transcript, if one is sitting beside it.
///
/// Nothing is transcribed here. A `.srt`, `.vtt`, or `.txt` with the same stem
/// is a transcript someone already produced, and reading it is the difference
/// between a video being a name in the graph and a video being searchable.
fn from_sidecar_transcript(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    let directory = path.parent()?;
    for suffix in ["srt", "vtt", "txt"] {
        let candidate = directory.join(format!("{stem}.{suffix}"));
        if let Ok(text) = std::fs::read_to_string(&candidate) {
            return Some(format!(
                "transcript from {}: {}",
                candidate
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_default(),
                strip_cues(&text)
            ));
        }
    }
    None
}

/// Subtitle text without the timing scaffolding: cue numbers, `-->` ranges,
/// and the `WEBVTT` header carry no meaning to a reader of the graph.
fn strip_cues(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty()
                && !line.contains("-->")
                && *line != "WEBVTT"
                && !line.chars().all(|character| character.is_ascii_digit())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn scratch(name: &str) -> std::path::PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("aag-extract-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        root.join(name)
    }

    /// Builds a minimal but real OOXML package: a zip with the part inside.
    fn zipped(path: &std::path::Path, entries: &[(&str, &str)]) {
        let file = std::fs::File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        for (name, body) in entries {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(body.as_bytes()).unwrap();
        }
        writer.finish().unwrap();
    }

    #[test]
    fn a_docx_gives_up_its_paragraphs() {
        let path = scratch("spec.docx");
        zipped(
            &path,
            &[(
                "word/document.xml",
                "<?xml version=\"1.0\"?><w:document xmlns:w=\"x\"><w:body>\
                 <w:p><w:r><w:t>The </w:t><w:r><w:t>Widget</w:t></w:r></w:r></w:p>\
                 <w:p><w:r><w:t>is built by build_widget.</w:t></w:r></w:p>\
                 </w:body></w:document>",
            )],
        );

        let text = text(&path).expect("text from a docx");

        assert!(text.contains("The Widget"), "{text}");
        assert!(text.contains("build_widget"), "{text}");
    }

    #[test]
    fn a_pptx_reads_every_slide_in_order() {
        let path = scratch("deck.pptx");
        zipped(
            &path,
            &[
                (
                    "ppt/slides/slide1.xml",
                    "<p:sld xmlns:a=\"x\"><a:p><a:t>Architecture</a:t></a:p></p:sld>",
                ),
                (
                    "ppt/slides/slide2.xml",
                    "<p:sld xmlns:a=\"x\"><a:p><a:t>Resolver ladder</a:t></a:p></p:sld>",
                ),
                (
                    "ppt/notesSlides/notesSlide1.xml",
                    "<p:notes>hidden</p:notes>",
                ),
            ],
        );

        let text = text(&path).expect("text from a pptx");

        assert!(text.contains("Architecture"), "{text}");
        assert!(text.contains("Resolver ladder"), "{text}");
        assert!(!text.contains("hidden"), "notes are not slides: {text}");
    }

    #[test]
    fn an_svg_contributes_its_labels() {
        let path = scratch("diagram.svg");
        std::fs::write(
            &path,
            "<svg xmlns=\"http://www.w3.org/2000/svg\"><text>Store</text>\
             <text>calls Graph</text></svg>",
        )
        .unwrap();

        let text = text(&path).expect("text from an svg");

        assert!(
            text.contains("Store") && text.contains("calls Graph"),
            "{text}"
        );
    }

    #[test]
    fn a_video_is_read_through_the_transcript_beside_it() {
        let path = scratch("walkthrough.mp4");
        std::fs::write(&path, [0u8; 8]).unwrap();
        std::fs::write(
            path.with_extension("srt"),
            "1\n00:00:01,000 --> 00:00:04,000\nThe Store type owns the connection.\n\n\
             2\n00:00:04,000 --> 00:00:07,000\nGraph::open is the entry point.\n",
        )
        .unwrap();

        let text = text(&path).expect("transcript beside the video");

        assert!(
            text.contains("The Store type owns the connection."),
            "{text}"
        );
        assert!(text.contains("Graph::open"), "{text}");
        assert!(!text.contains("-->"), "timings are scaffolding: {text}");
    }

    #[test]
    fn a_video_with_no_transcript_reads_as_nothing_rather_than_as_a_guess() {
        let path = scratch("silent.mp4");
        std::fs::write(&path, [0u8; 8]).unwrap();

        assert_eq!(text(&path), None);
    }

    #[test]
    fn an_unreadable_document_is_none_and_not_an_error() {
        let path = scratch("broken.docx");
        std::fs::write(&path, b"this is not a zip").unwrap();

        assert_eq!(text(&path), None);
    }

    #[test]
    fn extracted_text_is_bounded() {
        let path = scratch("long.csv");
        std::fs::write(&path, "word ".repeat(MAX_TEXT)).unwrap();

        let text = text(&path).expect("text from a csv");

        assert!(text.contains("text truncated"), "the cut is announced");
        assert!(text.chars().count() < MAX_TEXT + 100);
    }

    /// The fixture is a real PDF produced by a word processor, not a
    /// hand-written one: the point of this test is that an ordinary document
    /// gives up its text, and a PDF assembled to be easy would not show that.
    #[test]
    fn a_pdf_gives_up_its_text_layer() {
        let path = scratch("note.pdf");
        std::fs::write(&path, include_bytes!("../assets/tests/note.pdf")).unwrap();

        let text = text(&path).expect("text from a pdf");

        assert!(
            text.contains("The Widget type is built by build_widget."),
            "{text}"
        );
        assert!(
            text.contains("Graph::open is the entry point"),
            "every paragraph, not just the first: {text}"
        );
        assert!(text.contains("resolve_calls"), "{text}");
    }

    #[test]
    fn a_spreadsheet_contributes_its_sheet_names_and_cells() {
        // Written by the same library that reads it back, which is the only
        // way to build a real xlsx here; the assertion is about what the
        // reader surfaces, not about the writer.
        let path = scratch("config.xlsx");
        let mut workbook = rust_xlsxwriter::Workbook::new();
        let sheet = workbook.add_worksheet();
        sheet.set_name("Endpoints").unwrap();
        sheet.write_string(0, 0, "handler").unwrap();
        sheet.write_string(0, 1, "route").unwrap();
        sheet.write_string(1, 0, "listPets").unwrap();
        sheet.write_string(1, 1, "/pets").unwrap();
        workbook.save(&path).unwrap();

        let text = text(&path).expect("text from a workbook");

        assert!(
            text.contains("Endpoints"),
            "the sheet name is content: {text}"
        );
        assert!(
            text.contains("listPets") && text.contains("/pets"),
            "{text}"
        );
    }

    #[test]
    fn the_extractable_set_is_the_one_the_indexer_asks_about() {
        assert!(is_extractable("docs/spec.pdf"));
        assert!(is_extractable("DOCS/Spec.PDF"), "case is not a format");
        assert!(is_extractable("sheet.xlsx"));
        assert!(!is_extractable("src/main.rs"));
        assert!(!is_extractable("notes.md"), "a text doc is read directly");
    }
}
