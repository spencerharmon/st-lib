pub type OwnedMidiBytes = Vec<u8>;

#[derive(Debug)]
pub struct OwnedMidi {
    pub time: u32,
    pub bytes: OwnedMidiBytes
}
