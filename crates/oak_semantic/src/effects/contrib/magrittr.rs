use crate::effects::contrib::assign_op;
use crate::effects::contrib::Entry;
use crate::effects::TargetAccess::ReadWrite;

pub(crate) static ENTRIES: &[Entry] = &[assign_op!("%<>%", ReadWrite)];
