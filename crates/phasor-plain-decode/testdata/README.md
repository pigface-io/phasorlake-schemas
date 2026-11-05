Heads of AEMO's files, a few rows each, in the forms this crate reads: one
GBB file with a byte-order mark and day-first dates, one GBB cumulative file
in the slash form, one STTM and one DWGM report with `DD Mon YYYY` dates, a
WEM pre-reform datafile with units in its headers, a WEM post-reform file
with quoted day-first times, and a WEMDE document. Data © AEMO, used under
its general permission with attribution.

The `nz_*` files are heads of the Electricity Authority of New Zealand's
EMI datasets: a day's final energy prices, a day's dispatch energy prices
(instants in `+13:00`), and a month's metered generation (a column per
trading period, the day's last two empty). Data from www.emi.ea.govt.nz,
provided by the Electricity Authority (New Zealand), under CC BY 4.0.
