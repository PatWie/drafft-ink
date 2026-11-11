//! The system file dialogs (`rfd`) and the clipboard text read, off the UI thread.
//!
//! Every job runs on its own short-lived thread, so a dialog never blocks the event loop. The answer goes
//! through the inbox, which wakes the loop.

use std::path::PathBuf;

use drafftink_engine::ports::FileRef;

use crate::platform::{Host, HostJob, Inbound, Inbox, PickKind, PickedFile};

/// Opens dialogs on a worker thread.
#[derive(Debug, Default)]
pub(crate) struct NativeHost;

const DOCUMENT_EXTENSIONS: &[&str] = &["drafftink", "json", "excalidraw", "png"];
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp"];
const LIBRARY_EXTENSIONS: &[&str] = &["excalidrawlib", "json"];

fn path_ref(p: PathBuf) -> PickedFile {
    PickedFile::Ref(FileRef::Path { path: p.display().to_string() })
}

async fn pick(kind: PickKind, inbox: &Inbox) {
    let (title, name, exts) = match kind {
        PickKind::Document => ("Open a document", "Documents", DOCUMENT_EXTENSIONS),
        PickKind::Images => ("Insert pictures", "Pictures", IMAGE_EXTENSIONS),
        PickKind::Library => ("Open a library", "Excalidraw libraries", LIBRARY_EXTENSIONS),
        PickKind::LibraryImport => ("Import a library", "Excalidraw libraries", LIBRARY_EXTENSIONS),
    };
    let dialog = rfd::AsyncFileDialog::new().set_title(title).add_filter(name, exts);
    let handles = match kind {
        PickKind::Images => dialog.pick_files().await.unwrap_or_default(),
        _ => dialog.pick_file().await.into_iter().collect(),
    };
    if handles.is_empty() {
        return;
    }
    let files: Vec<PickedFile> = match kind {
        // A document or library keeps its path: it becomes the save target.
        PickKind::Document | PickKind::Library | PickKind::LibraryImport => {
            handles.iter().map(|h| path_ref(h.path().to_owned())).collect()
        }
        // Pictures are read now; the document does not remember where they came from.
        PickKind::Images => {
            let mut out = Vec::new();
            for h in &handles {
                out.push(PickedFile::Bytes { name: h.file_name(), bytes: h.read().await });
            }
            out
        }
    };
    inbox.post(Inbound::Picked { kind, files });
}

impl Host for NativeHost {
    fn start(&mut self, job: HostJob, reply: Inbox) {
        let spawned = std::thread::Builder::new().name("drafftink-dialog".to_owned()).spawn(move || match job {
            HostJob::PickDocument => pollster::block_on(pick(PickKind::Document, &reply)),
            HostJob::PickImages => pollster::block_on(pick(PickKind::Images, &reply)),
            HostJob::PickLibrary => pollster::block_on(pick(PickKind::Library, &reply)),
            HostJob::PickLibraryImport => pollster::block_on(pick(PickKind::LibraryImport, &reply)),
            HostJob::PickSaveTarget { suggested } => {
                let dialog = rfd::AsyncFileDialog::new()
                    .set_title("Save as")
                    .set_file_name(suggested)
                    .add_filter("Drafft.ink document", &["drafftink"]);
                if let Some(h) = pollster::block_on(dialog.save_file()) {
                    reply.post(Inbound::SaveTarget(FileRef::Path { path: h.path().display().to_string() }));
                }
            }
            HostJob::LoadFont(file) => {
                reply.post(Inbound::Notice(format!(
                    "The font {} is built into the desktop app; nothing to load.",
                    file.file_name()
                )));
            }
            HostJob::ReadClipboardText => match arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
                Ok(text) => reply.post(Inbound::ClipboardText(text)),
                Err(e) => reply.post(Inbound::Notice(format!("The clipboard has no text to paste ({e})."))),
            },
        });
        if let Err(e) = spawned {
            log::warn!("could not start a dialog thread: {e}");
        }
    }
}
