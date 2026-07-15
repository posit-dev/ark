use crate::effects::contrib::custom;
use crate::effects::contrib::Entry;
use crate::effects::BindingOperatorHandler;

pub(crate) fn entries() -> Vec<Entry> {
    vec![custom("S7", ":=", &BindingOperatorHandler)]
}
