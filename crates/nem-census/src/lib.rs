//! The census of AEMO's MMS archive: which shape every block of every file
//! of every month had.
//!
//! The readers it is built on:
//!
//! - [`zip`] walks an archived zip, zips inside zips included, and hands
//!   each `.CSV` entry over as a stream;
//! - [`mms`] scans a CSV's C, I and D rows into a [`mms::Sink`];
//! - [`ctl`] parses the SQL*Loader control file that says what type AEMO
//!   loads each column as;
//! - [`names`] reads a table and [`month::YearMonth`] out of an archive
//!   file name;
//! - [`shape`] gives a block's report pair and columns a content-addressed
//!   [`shape::ShapeId`], the same id utilake gives it.
//!
//! Std, serde, SHA-256, xxh3 and the zip reader only.

pub mod ctl;
pub mod mms;
pub mod month;
pub mod names;
pub mod shape;
pub mod text;
pub mod zip;
