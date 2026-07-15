use crate::effects::contrib::assign_op;
use crate::effects::contrib::Entry;
use crate::effects::TargetAccess::Write;

pub(crate) static ENTRIES: &[Entry] = &[assign_op!("S7", ":=", Write)];
