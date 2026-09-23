//! Core analysis for pyprojx: parsing, diagnostics, schemas, and version detection.
//!
//! This crate must stay free of CLI concerns such as argument parsing and
//! output rendering so that other front ends (a language server, or Python
//! bindings) can reuse it. No analysis is implemented yet.
