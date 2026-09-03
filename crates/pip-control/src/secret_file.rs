use std::fs::Metadata;
use std::os::unix::fs::{MetadataExt, PermissionsExt};

pub(crate) fn metadata_allows_secret_read(uid: u32, gid: u32, mode: u32) -> bool {
    let mode = mode & 0o777;
    // systemd exposes LoadCredential files as root:root 0440 and grants the
    // service identity access through the credential mount rather than the
    // file's group membership.
    mode & 0o077 == 0 || (uid == 0 && gid == 0 && mode == 0o440)
}

pub(crate) fn metadata_is_safe_secret_file(metadata: &Metadata) -> bool {
    !metadata.file_type().is_symlink()
        && metadata.is_file()
        && metadata_allows_secret_read(
            metadata.uid(),
            metadata.gid(),
            metadata.permissions().mode(),
        )
}

#[cfg(test)]
mod tests {
    use super::metadata_allows_secret_read;

    #[test]
    fn accepts_owner_private_and_exact_systemd_credential_modes() {
        assert!(metadata_allows_secret_read(501, 20, 0o600));
        assert!(metadata_allows_secret_read(0, 0, 0o440));
    }

    #[test]
    fn rejects_other_group_readable_or_public_secret_files() {
        assert!(!metadata_allows_secret_read(501, 20, 0o440));
        assert!(!metadata_allows_secret_read(0, 20, 0o440));
        assert!(!metadata_allows_secret_read(0, 0, 0o444));
        assert!(!metadata_allows_secret_read(0, 0, 0o640));
    }
}
