# Band plans

Reference band plans in the SDR++ JSON schema (Phase 10.14, D16), copied
unmodified from SDR++ (`root/res/bandplans/`, commit
`8c9f5ee8fe405775bfcd62c8c8f8c0fc928a64af`), by Alexandre Rouma and the
plan authors named in each file, under the GNU General Public License v3.

Some upstream plans carry typos (a band whose end is below its start, e.g.
`italy.json` "GSM-R"). The files are kept byte-identical to upstream;
`neowon-refdb` drops such bands at load and reports them instead of guessing
a correction.

Your own plans go in `~/.neowon/bandplans/*.json` (same schema) and shadow a
shipped plan with the same file name.
