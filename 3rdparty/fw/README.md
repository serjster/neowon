# Third-party firmware: OWON VDS1022 FPGA bitstreams

The files `VDS1022_FPGAV*.bin` in this directory are **OWON's proprietary
FPGA bitstreams**, extracted from the OWON VDS1022 1.1.x PC software
(http://www.owon.com.hk) as redistributed by the community repository
[OWON-VDS1022](https://github.com/florentbr/OWON-VDS1022) (`fwr/`).

They are **not covered by this repository's MIT/Apache-2.0 license** and
remain the property of Lilliput/OWON. They are vendored here solely so
that neowon works out of the box with the instrument they belong to; if
you are the rights holder and want them removed, open an issue and they
will be taken down immediately.

The scope uploads one of these at every cold start (the FPGA is volatile).
neowon picks the file matching the unit's hardware version
(`VDS1022_FPGAV{n}_*.bin`, e.g. hw V5.x → `FPGAV5`). Search order:
`$NEOWON_FPGA_DIR`, `./fwr`, `./3rdparty/fw`, `../OWON-VDS1022/fwr`.
