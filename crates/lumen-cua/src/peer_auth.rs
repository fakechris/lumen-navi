//! macOS peer-code validation for the local capability socket.

#[cfg(not(test))]
use std::ffi::c_void;
#[cfg(not(test))]
use std::os::fd::AsRawFd;

use anyhow::Result;

#[cfg(test)]
pub(crate) fn authorize_peer(_stream: &tokio::net::UnixStream) -> Result<()> {
    // Unit-test binaries are intentionally unsigned. Production builds always
    // execute the dynamic code-signing validation below.
    Ok(())
}

#[cfg(not(test))]
pub(crate) fn authorize_peer(stream: &tokio::net::UnixStream) -> Result<()> {
    use anyhow::{bail, Context};
    use core_foundation::base::TCFType;
    use core_foundation::data::CFData;
    use security_framework::os::macos::code_signing::{
        Flags, GuestAttributes, SecCode, SecRequirement,
    };

    let mut audit_token = [0u32; 8];
    let mut token_len = std::mem::size_of_val(&audit_token) as libc::socklen_t;
    let status = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERTOKEN,
            audit_token.as_mut_ptr().cast::<c_void>(),
            &mut token_len,
        )
    };
    if status != 0 || token_len as usize != std::mem::size_of_val(&audit_token) {
        bail!("could not read the Lumen Cua peer audit token");
    }

    let token_bytes = unsafe {
        std::slice::from_raw_parts(
            audit_token.as_ptr().cast::<u8>(),
            std::mem::size_of_val(&audit_token),
        )
    };
    let token_data = CFData::from_buffer(token_bytes);
    let mut attributes = GuestAttributes::new();
    attributes.set_audit_token(token_data.as_concrete_TypeRef());
    let peer = SecCode::copy_guest_with_attribues(None, &attributes, Flags::NONE)
        .context("resolve Lumen Cua peer code")?;

    let own_code = SecCode::for_self(Flags::NONE).context("resolve Lumen Cua code identity")?;
    let own_requirement = unsafe { designated_requirement_text(&own_code)? };
    if !own_requirement.contains("certificate ") && !own_requirement.contains("anchor apple") {
        bail!("Lumen Cua requires a certificate-backed code signature");
    }

    for identifier in ["com.lumenopen.navi", "lumen-daemon"] {
        let requirement_text = own_requirement.replacen(
            "identifier \"com.lumenopen.cua\"",
            &format!("identifier \"{identifier}\""),
            1,
        );
        if requirement_text == own_requirement {
            bail!("unexpected Lumen Cua designated requirement");
        }
        let requirement: SecRequirement = requirement_text
            .parse()
            .context("compile Lumen Cua client requirement")?;
        if peer.check_validity(Flags::NONE, &requirement).is_ok() {
            return Ok(());
        }
    }
    bail!("Lumen Cua rejected a client not signed by the same Lumen identity")
}

#[cfg(not(test))]
unsafe fn designated_requirement_text(
    code: &security_framework::os::macos::code_signing::SecCode,
) -> Result<String> {
    use core_foundation::base::{CFRelease, TCFType};
    use core_foundation::string::{CFString, CFStringRef};

    let mut requirement: *mut c_void = std::ptr::null_mut();
    let status = unsafe {
        SecCodeCopyDesignatedRequirement(
            code.as_concrete_TypeRef().cast::<c_void>(),
            0,
            &mut requirement,
        )
    };
    if status != 0 || requirement.is_null() {
        anyhow::bail!("copy Lumen Cua designated requirement failed ({status})");
    }
    let mut text_ref: CFStringRef = std::ptr::null();
    let string_status = unsafe { SecRequirementCopyString(requirement, 0, &mut text_ref) };
    unsafe { CFRelease(requirement.cast_const()) };
    if string_status != 0 || text_ref.is_null() {
        anyhow::bail!("format Lumen Cua designated requirement failed ({string_status})");
    }
    Ok(unsafe { CFString::wrap_under_create_rule(text_ref) }.to_string())
}

#[cfg(not(test))]
#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    fn SecCodeCopyDesignatedRequirement(
        code: *mut c_void,
        flags: u32,
        requirement: *mut *mut c_void,
    ) -> i32;
    fn SecRequirementCopyString(
        requirement: *mut c_void,
        flags: u32,
        text: *mut core_foundation::string::CFStringRef,
    ) -> i32;
}
