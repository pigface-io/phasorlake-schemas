//! The census of AEMO's MMS archive: which shape every block of every file
//! of every month had.
//!
//! - [`fetch`] downloads one month of the archive, politely, and records
//!   where each file came from;
//! - [`census`] reads every file of a month's directory into its census;
//!   [`reports`] does the same for the report folders, as runs;
//! - [`catalog`] is the hand-kept declaration of each table: the table of
//!   a report pair, its key and its time column;
//! - [`models`] records the data-model releases the archive carries, read
//!   by [`ddl`];
//! - [`fold`] folds every census into each table's [`history`] as an
//!   Iceberg table, kept to the history last published;
//! - [`export`] writes each history as the documents under `nem/`, and
//!   [`compat`] and [`diff`] hold them to what a git revision
//!   [`published`];
//! - [`sql`] plans a read of an interval of a table's files, and
//!   [`iceberg`] writes each history as an Iceberg table without data.
//!
//! The readers they are built on:
//!
//! - [`zip`] walks an archived zip, zips inside zips included, and hands
//!   each `.CSV` entry over as a stream;
//! - [`mms`] scans a CSV's C, I and D rows into a [`mms::Sink`];
//! - [`profile`] says what each column's values were;
//! - [`ctl`] parses the SQL*Loader control file that says what type AEMO
//!   loads each column as;
//! - [`names`] reads a table and [`month::YearMonth`] out of an archive
//!   file name;
//! - [`shape`] gives a block's report pair and columns a content-addressed
//!   [`shape::ShapeId`], the same id utilake gives it.

pub mod catalog;
pub mod census;
pub mod compat;
pub mod ctl;
pub mod ddl;
pub mod diff;
pub mod export;
pub mod fetch;
pub mod files;
pub mod fold;
pub mod history;
pub mod iceberg;
pub mod mms;
pub mod models;
pub mod month;
pub mod names;
pub mod pool;
pub mod profile;
pub mod published;
pub mod reports;
pub mod shape;
pub mod sql;
pub mod text;
pub mod zip;
