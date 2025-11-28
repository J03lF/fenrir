use super::manifest::ModuleManifest;

#[derive(Debug, Clone)]
pub struct ModuleBundle {
    pub manifest: ModuleManifest,
    pub archive: Vec<u8>,
    pub signature: Vec<u8>,
    pub checksum: Vec<u8>,
}
