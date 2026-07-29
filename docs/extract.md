---
wiki: src/extract.rs
---

# extract.rs

Native text and metadata extraction for binary documents. This is P1.9 of
[capability coverage](capability-coverage.md).

A PDF, a `.docx`, a spreadsheet, or a screenshot used to enter the graph as a
`Doc` node with no description, waiting for a host agent to look at it. That
path works and it stays — but it costs a vision pass per file, and it only ever
happens for files an agent happens to open. Most of these formats carry their
text in the open: a `.docx` is a zip of XML, a spreadsheet is a table of
strings, a PDF has a text layer unless it is a scan.

So indexing reads them first. What comes back becomes the doc's description and
goes through the same linking as a `.md` file, which means a design PDF that
names `build_widget` gets an `Explains` edge into it, in the first pass, with no
agent involved.

## What is read, and from where

| Format | Source of text |
|---|---|
| `.pdf` | the text layer, via `pdf-extract` |
| `.docx` | `word/document.xml`, one line per `w:p` |
| `.pptx` | every `ppt/slides/slideN.xml` in order, one line per `a:p` — notes are not slides |
| `.odt`, `.odp` | `content.xml`, one line per `text:p` |
| `.xlsx`, `.xlsm`, `.xls`, `.ods` | every sheet: its name, then its non-empty cells |
| `.csv` | the file |
| `.svg` | its `<text>` labels — most of what a diagram contributes |
| `.png`, `.jpg`, `.gif`, `.webp` | dimensions, plus authoring EXIF (`Software`, `Artist`, `ImageDescription`, `XPTitle`, …) |
| `.mp4`, `.mov`, `.avi`, `.mkv`, `.webm` | the `.srt`/`.vtt`/`.txt` transcript sitting beside it, cue numbers and timings stripped |

`.srt` and `.vtt` are also indexed as text documents in their own right, as are
`.rst` and `.adoc`.

Extraction is bounded at 40 000 characters and says so in the text when it cuts:
a doc's text lands in a node description that an agent reads, and a 400-page PDF
pasted whole would crowd out everything else.

## What is not read

- **No OCR.** A scanned PDF and an unlabelled screenshot have no text to read,
  and `None` is the honest answer. The node keeps the empty description
  `aag describe` expects to find, so the host-agent path is exactly where it
  was.
- **No speech recognition.** A video contributes the transcript someone already
  produced. With no sidecar, it contributes nothing — the graph does not
  pretend to have watched it.
- **An image's metadata is not its content.** `1920×1080 pixels; Software:
  Figma` says where a picture came from, not what it shows. A vision pass still
  beats it, and `aag describe` overwrites it when one runs.
- **A malformed file is skipped, not fatal.** `pdf-extract` panics on some
  broken PDFs rather than erroring, so the call is caught; one bad document
  never takes down an index pass.

## Where it sits

`crate::resolve::index_doc_file` calls `extract::text` for every binary doc.
That is the only integration point: everything downstream — description,
`Explains` edges, FTS, the wiki, the site — already handles a doc that has text,
because a `.md` file has always been one.
