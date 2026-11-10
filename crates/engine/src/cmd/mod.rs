//! Command families. Each family is `cmd/<family>.rs` with `pub const COMMANDS: &[CommandSpec]` and its
//! tests in `cmd/<family>_tests.rs` (included from the family file). To add a family, create the files
//! and add its name to the list below: that line is the only edit to a shared file.

use crate::spec::CommandSpec;

/// Declares each family module and builds the registry list from the same names.
macro_rules! families {
    ($($name:ident),* $(,)?) => {
        $(pub mod $name;)*
        /// Every family's commands, in registry order.
        pub const FAMILIES: &[&[CommandSpec]] = &[$($name::COMMANDS),*];
    };
}

pub(crate) mod element_common;

families! { arrange, arrow, clipboard, collab, diagram, doc, doc_read, element, element_create, element_query, engine, frame, gesture, group, history, image, layout, library, perf, selection, settings, style, tab, text, tool, view }
