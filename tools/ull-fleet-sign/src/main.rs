//! Offline signing for OTA images.
//!
//! ```text
//! ull-fleet-sign keygen <secret-key-file>          create a key; prints the public key
//! ull-fleet-sign pubkey <secret-key-file>          print the public key again
//! ull-fleet-sign sign <secret-key-file> <image>    print the signature for an image
//! ull-fleet-sign verify <public-key-hex> <image> <signature-hex>
//! ull-fleet-sign inspect <image>                   print project, version and build info
//! ```
//!
//! The secret key file holds the 32-byte Ed25519 seed as hex. Keep it off the
//! fleet server: the server only stores and forwards signatures.

use std::fs;
use std::io::{Read, Write};
use std::path::Path;

use anyhow::{Context, bail};
use ull_fleet_sign::{from_hex, public_key, sign_image, to_hex, verify_image};

const USAGE: &str = "usage:
  ull-fleet-sign keygen <secret-key-file>
  ull-fleet-sign pubkey <secret-key-file>
  ull-fleet-sign sign <secret-key-file> <image>
  ull-fleet-sign verify <public-key-hex> <image> <signature-hex>
  ull-fleet-sign inspect <image>";

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();

    match args.as_slice() {
        ["keygen", key_path] => keygen(Path::new(key_path)),
        ["pubkey", key_path] => {
            println!(
                "{}",
                to_hex(&public_key(&read_secret(Path::new(key_path))?))
            );
            Ok(())
        }
        ["sign", key_path, image_path] => {
            let seed = read_secret(Path::new(key_path))?;
            let image = read_image(Path::new(image_path))?;
            println!("{}", to_hex(&sign_image(&seed, &image)));
            Ok(())
        }
        ["verify", public_key_hex, image_path, signature_hex] => {
            let image = read_image(Path::new(image_path))?;
            verify_image(
                &from_hex(public_key_hex).context("public key")?,
                &image,
                &from_hex(signature_hex).context("signature")?,
            )?;
            println!("ok");
            Ok(())
        }
        ["inspect", image_path] => {
            let image = read_image(Path::new(image_path))?;
            let desc = ull_fleet_sign::appdesc::AppDesc::parse(&image)?;
            println!("project         {}", desc.project_name);
            println!("version         {}", desc.version);
            println!("secure_version  {}", desc.secure_version);
            println!("built           {} {}", desc.date, desc.time);
            println!("size            {} bytes", image.len());
            println!(
                "sha256          {}",
                to_hex(&ull_fleet_sign::sha256(&image))
            );
            Ok(())
        }
        _ => bail!("{USAGE}"),
    }
}

fn keygen(path: &Path) -> anyhow::Result<()> {
    let mut seed = [0u8; 32];
    fs::File::open("/dev/urandom")
        .and_then(|mut random| random.read_exact(&mut seed))
        .context("failed to read /dev/urandom")?;

    write_secret(path, &to_hex(&seed))?;
    eprintln!("wrote secret key to {}", path.display());
    println!("{}", to_hex(&public_key(&seed)));
    Ok(())
}

#[cfg(unix)]
fn write_secret(path: &Path, contents: &str) -> anyhow::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .with_context(|| {
            format!(
                "failed to create {} (it must not exist yet)",
                path.display()
            )
        })?;
    writeln!(file, "{contents}").context("failed to write secret key")
}

#[cfg(not(unix))]
fn write_secret(path: &Path, contents: &str) -> anyhow::Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| {
            format!(
                "failed to create {} (it must not exist yet)",
                path.display()
            )
        })?;
    writeln!(file, "{contents}").context("failed to write secret key")
}

fn read_secret(path: &Path) -> anyhow::Result<[u8; 32]> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("failed to read secret key {}", path.display()))?;
    from_hex(&contents).with_context(|| format!("invalid secret key in {}", path.display()))
}

fn read_image(path: &Path) -> anyhow::Result<Vec<u8>> {
    fs::read(path).with_context(|| format!("failed to read image {}", path.display()))
}
