use compact_str::CompactString;

/// Interned identifier.
///
/// Lets tracked queries cache symbols or packages by name cheaply.
/// Used by [`crate::SourceGraph::package_by_name`] and by
/// [`crate::File::resolve`] so repeated calls for the same name hit
/// the salsa cache.
///
/// The text is stored as `CompactString` so short identifiers stay inline
/// (up to 24 bytes on 64-bit, 12 on 32-bit). R symbols and package names
/// are almost always ASCII and short enough to fit.
#[salsa::interned]
pub struct Name<'db> {
    #[returns(ref)]
    pub text: CompactString,
}
