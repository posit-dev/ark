/// Interned identifier.
///
/// Lets tracked queries cache symbols or packages by name cheaply.
#[salsa::interned]
pub struct Name<'db> {
    #[returns(ref)]
    pub text: String,
}
