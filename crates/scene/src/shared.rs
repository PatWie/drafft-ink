//! A document at one moment, held without copying it, and written as JSON a piece at a time.
//!
//! Writing a document of 100,000 elements takes 0.2 s of work (a copy, the validation and 45 MB of JSON) before a byte
//! reaches the disk. A frame cannot wait for that, so a long job takes a [`SharedDocument`] (a list of pointers to the
//! store's elements, see [`crate::MemoryStore::share_all`]) and a [`JsonJob`] turns it into the text of
//! [`Document::to_json`] in slices of a bounded size. The store goes on changing meanwhile; the job sees the
//! moment it started.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::color::Color;
use crate::document::{Document, DocumentError, FileEntry};
use crate::element::Element;
use crate::id::FileId;
use crate::validate::{Validator, validate_parts};

/// The content of a document, shared with the store it was taken from.
#[derive(Debug, Clone)]
pub struct SharedDocument {
    /// Document name.
    pub name: String,
    /// Canvas background.
    pub background: Color,
    /// All elements, tombstones included, back to front.
    pub elements: Vec<Arc<Element>>,
    /// Image blobs by content hash (the bytes are shared, too).
    pub files: BTreeMap<FileId, FileEntry>,
}

impl SharedDocument {
    /// The rules of [`Document::to_json`]: the text is only written for a document that can be read back.
    /// One pass, about 20 ms at 100,000 elements.
    pub fn check(&self) -> Result<(), DocumentError> {
        let refs: Vec<&Element> = self.elements.iter().map(|e| &**e).collect();
        let report = validate_parts(&self.name, &refs, &self.files);
        if report.has_errors() { Err(DocumentError::Invalid(report)) } else { Ok(()) }
    }

    /// A check of this document in steps ([`DocumentCheck`]).
    pub fn checker(&self) -> DocumentCheck {
        DocumentCheck { validator: Validator::for_document(&self.name) }
    }

    /// The same content as an owned [`Document`] (a copy of every element).
    pub fn to_document(&self) -> Document {
        Document {
            name: self.name.clone(),
            background: self.background,
            elements: self.elements.iter().map(|e| (**e).clone()).collect(),
            files: self.files.clone(),
            ..Document::default()
        }
    }
}

/// [`SharedDocument::check`] a bounded piece at a time.
#[derive(Debug)]
pub struct DocumentCheck {
    validator: Validator,
}

impl DocumentCheck {
    /// Looks at up to `budget` more elements of `doc`, which must be the document the check was made for. True when
    /// the check is complete.
    pub fn step(&mut self, doc: &SharedDocument, budget: usize) -> bool {
        let files: Vec<_> = doc.files.iter().collect();
        self.validator.step(&doc.elements, &files, budget)
    }

    /// The verdict, as [`SharedDocument::check`] gives it. Call it when [`Self::step`] has answered true.
    ///
    /// # Errors
    /// When the document breaks a rule.
    pub fn finish(self) -> Result<(), DocumentError> {
        let report = self.validator.finish();
        if report.has_errors() { Err(DocumentError::Invalid(report)) } else { Ok(()) }
    }
}

/// Writes a [`SharedDocument`] as the compact JSON of [`Document::to_json`], a slice at a time. The text is the
/// same, byte for byte.
#[derive(Debug)]
pub struct JsonJob {
    doc: SharedDocument,
    out: Vec<u8>,
    next: usize,
    /// What follows the elements: the end of the array, the files and the end of the object.
    tail: Vec<u8>,
}

impl JsonJob {
    /// Starts a job. Nothing is written and nothing is checked yet.
    ///
    /// # Errors
    /// When a field of the document cannot be written as JSON.
    pub fn new(doc: SharedDocument) -> Result<Self, DocumentError> {
        let write = |e: serde_json::Error| DocumentError::Write(Arc::new(e));
        // The head and the tail come from the serialiser itself: the empty document ends in `"elements":[]}`.
        let empty = Document { name: doc.name.clone(), background: doc.background, ..Document::default() };
        let head = serde_json::to_string(&empty).map_err(write)?;
        let Some(head) = head.strip_suffix("]}") else {
            return Err(write(serde_json::Error::io(std::io::Error::other(
                "the document no longer ends in its elements",
            ))));
        };
        let mut out = Vec::with_capacity(head.len() + doc.elements.len().saturating_mul(400));
        out.extend_from_slice(head.as_bytes());
        let mut tail = b"]".to_vec();
        if !doc.files.is_empty() {
            tail.extend_from_slice(b",\"files\":");
            tail.extend_from_slice(serde_json::to_string(&doc.files).map_err(write)?.as_bytes());
        }
        tail.push(b'}');
        Ok(Self { doc, out, next: 0, tail })
    }

    /// The document being written.
    pub fn document(&self) -> &SharedDocument {
        &self.doc
    }

    /// Elements written so far.
    pub fn written(&self) -> usize {
        self.next
    }

    /// Bytes of text so far.
    pub fn len(&self) -> usize {
        self.out.len()
    }

    /// True before the first byte.
    pub fn is_empty(&self) -> bool {
        self.out.is_empty()
    }

    /// Writes elements until about `budget` bytes of text were added, or the last one. Returns the number of
    /// elements this slice wrote; the job is done when [`Self::is_done`].
    ///
    /// # Errors
    /// When an element cannot be written as JSON (a number that is not finite).
    pub fn step(&mut self, budget: usize) -> Result<usize, DocumentError> {
        let start = (self.next, self.out.len());
        while let Some(e) = self.doc.elements.get(self.next) {
            if self.next > 0 {
                self.out.push(b',');
            }
            serde_json::to_writer(&mut self.out, &**e).map_err(|e| DocumentError::Write(Arc::new(e)))?;
            self.next += 1;
            if self.out.len() - start.1 >= budget {
                break;
            }
        }
        Ok(self.next - start.0)
    }

    /// True when every element is written.
    pub fn is_done(&self) -> bool {
        self.next >= self.doc.elements.len()
    }

    /// The finished text.
    ///
    /// # Errors
    /// When elements are left to write.
    pub fn finish(mut self) -> Result<Vec<u8>, DocumentError> {
        if !self.is_done() {
            return Err(DocumentError::Write(Arc::new(serde_json::Error::io(std::io::Error::other(
                "the document is not written to its last element",
            )))));
        }
        self.out.extend_from_slice(&self.tail);
        Ok(self.out)
    }
}
