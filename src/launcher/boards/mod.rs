// SPDX-License-Identifier: GPL-3.0-or-later

//! Per-board launcher profile data. One module per supported board:
//! adding a board means adding a file here plus a `KnownVariant`
//! constructor in the parent module -- board data never lives inline
//! in shared launcher code.

pub(crate) mod proteus_f7;
