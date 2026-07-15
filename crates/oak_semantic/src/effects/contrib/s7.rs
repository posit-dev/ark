use crate::effects::contrib::assign_op;
use crate::effects::contrib::Entry;

pub(crate) static ENTRIES: &[Entry] = &[assign_op!("S7", ":=")];
