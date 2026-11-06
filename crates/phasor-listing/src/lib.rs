//! An IIS directory listing read two ways.
//!
//! ```html
//! <pre><A HREF="/x/">[To Parent Directory]</A><br><br>
//!  2/13/2024  8:27 AM      1584114 <A HREF="/x/PUBLIC_DVD_DISPATCHPRICE_202401010000.zip">PUBLIC_DVD_DISPATCHPRICE_202401010000.zip</A><br>
//!  2/13/2024  8:27 AM        &lt;dir&gt; <A HREF="/x/sub/">sub</A><br>
//! ```
//!
//! - [`lenient`]: every link with the size and time its line shows, in one
//!   pass with no allocation per line beyond the entry; a kit lists what
//!   to fetch with it, and odd markup is simply a link without a time.
//! - [`strict`]: every folder and file, or an [`strict::Unparsed`] error
//!   naming the first line that is not one; a mirror crawls with it, so
//!   a change in the format stops the crawl instead of thinning it.
//!
//! Times are read with [`phasor_provenance::Stamp::from_listing`], in the
//! clock the caller names (nemweb's listings are AEST).

pub mod lenient;
pub mod strict;
