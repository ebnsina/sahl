//! Where printer bytes go.

use std::io::Write as _;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::time::Duration;

use thiserror::Error;

/// How long to wait on a printer before giving up.
///
/// Short on purpose. A cashier is standing at a counter with a customer in front of them, and a
/// thirty-second stall is worse than a message saying the receipt did not print — they can reprint,
/// but they cannot un-wait.
const TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PrintError {
    #[error("no printer is configured")]
    NotConfigured,

    #[error("{target} is not a printer address: {reason}")]
    BadTarget { target: String, reason: String },

    #[error("could not reach the printer at {address}: {reason}")]
    Unreachable { address: String, reason: String },

    #[error("the printer accepted the connection but not the job: {0}")]
    WriteFailed(String),
}

/// Where a till sends receipts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrinterTarget {
    /// A network printer, almost always on port 9100 — the de facto raw ESC/POS port.
    Network(String),
    /// Append the raw bytes to a file.
    ///
    /// Not a mock. It is how the byte stream gets inspected without hardware, and it is genuinely
    /// useful in the field: a merchant reporting "the receipt looks wrong" can send the file rather
    /// than a photograph of a curled roll.
    File(PathBuf),
    /// No printer. A perfectly ordinary configuration — plenty of small shops do not print.
    None,
}

impl PrinterTarget {
    /// Parse the `SAHL_PRINTER` setting.
    ///
    /// Unset means no printer, which is valid. A *malformed* value is an error rather than a
    /// fallback to none: someone who typed an address wants printing, and silently not printing is
    /// the failure they would discover from a customer.
    ///
    /// # Errors
    /// [`PrintError::BadTarget`] if the value is not a form this understands.
    pub fn parse(value: Option<&str>) -> Result<Self, PrintError> {
        let Some(raw) = value.map(str::trim).filter(|value| !value.is_empty()) else {
            return Ok(Self::None);
        };

        if let Some(address) = raw.strip_prefix("tcp://") {
            if address.is_empty() {
                return Err(PrintError::BadTarget {
                    target: raw.to_owned(),
                    reason: "no host".to_owned(),
                });
            }
            // Default to 9100 when no port is given, since that is the only port these speak.
            let address = if address.contains(':') {
                address.to_owned()
            } else {
                format!("{address}:9100")
            };
            return Ok(Self::Network(address));
        }

        if let Some(path) = raw.strip_prefix("file://") {
            if path.is_empty() {
                return Err(PrintError::BadTarget {
                    target: raw.to_owned(),
                    reason: "no path".to_owned(),
                });
            }
            return Ok(Self::File(PathBuf::from(path)));
        }

        Err(PrintError::BadTarget {
            target: raw.to_owned(),
            reason: "expected tcp://host[:port] or file:///path".to_owned(),
        })
    }

    #[must_use]
    pub const fn is_configured(&self) -> bool {
        !matches!(self, Self::None)
    }
}

/// Send a rendered job.
///
/// # Errors
/// [`PrintError`] describing what went wrong. Callers surface it; none of them undo a sale over it.
pub fn print(target: &PrinterTarget, bytes: &[u8]) -> Result<(), PrintError> {
    match target {
        PrinterTarget::None => Err(PrintError::NotConfigured),

        PrinterTarget::File(path) => {
            // Appended, not overwritten: the file is a spool of the day, and a reprint that erased
            // the previous receipt would destroy the only record of what the paper actually said.
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .map_err(|error| PrintError::Unreachable {
                    address: path.display().to_string(),
                    reason: error.to_string(),
                })?;
            file.write_all(bytes)
                .map_err(|error| PrintError::WriteFailed(error.to_string()))?;
            file.flush()
                .map_err(|error| PrintError::WriteFailed(error.to_string()))
        }

        PrinterTarget::Network(address) => {
            let resolved = address
                .to_socket_addrs()
                .map_err(|error| PrintError::Unreachable {
                    address: address.clone(),
                    reason: error.to_string(),
                })?
                .next()
                .ok_or_else(|| PrintError::Unreachable {
                    address: address.clone(),
                    reason: "resolved to no address".to_owned(),
                })?;

            // Connect *with* a timeout rather than connecting and then setting one: a printer that
            // is switched off usually drops packets rather than refusing, so a plain connect hangs
            // for the OS default, which is far longer than anyone will stand at a counter.
            let mut stream = TcpStream::connect_timeout(&resolved, TIMEOUT).map_err(|error| {
                PrintError::Unreachable {
                    address: address.clone(),
                    reason: error.to_string(),
                }
            })?;
            stream
                .set_write_timeout(Some(TIMEOUT))
                .map_err(|error| PrintError::WriteFailed(error.to_string()))?;

            stream
                .write_all(bytes)
                .map_err(|error| PrintError::WriteFailed(error.to_string()))?;
            stream
                .flush()
                .map_err(|error| PrintError::WriteFailed(error.to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unset_printer_is_a_valid_configuration() {
        // Plenty of small shops do not print at all.
        assert_eq!(PrinterTarget::parse(None), Ok(PrinterTarget::None));
        assert_eq!(PrinterTarget::parse(Some("")), Ok(PrinterTarget::None));
        assert_eq!(PrinterTarget::parse(Some("   ")), Ok(PrinterTarget::None));
    }

    #[test]
    fn a_network_printer_defaults_to_the_raw_escpos_port() {
        assert_eq!(
            PrinterTarget::parse(Some("tcp://192.168.1.50")),
            Ok(PrinterTarget::Network("192.168.1.50:9100".to_owned()))
        );
        assert_eq!(
            PrinterTarget::parse(Some("tcp://192.168.1.50:9101")),
            Ok(PrinterTarget::Network("192.168.1.50:9101".to_owned()))
        );
    }

    #[test]
    fn a_file_target_is_parsed() {
        assert_eq!(
            PrinterTarget::parse(Some("file:///tmp/receipts.bin")),
            Ok(PrinterTarget::File(PathBuf::from("/tmp/receipts.bin")))
        );
    }

    #[test]
    fn a_malformed_target_is_an_error_not_a_silent_none() {
        // Someone who typed an address wants printing. Falling back to "no printer" is the failure
        // they would discover from a customer asking for a receipt.
        for bad in ["192.168.1.50", "http://printer", "tcp://", "file://"] {
            assert!(
                matches!(
                    PrinterTarget::parse(Some(bad)),
                    Err(PrintError::BadTarget { .. })
                ),
                "accepted {bad}"
            );
        }
    }

    #[test]
    fn printing_with_no_printer_configured_says_so() {
        assert!(matches!(
            print(&PrinterTarget::None, b"hello"),
            Err(PrintError::NotConfigured)
        ));
    }

    #[test]
    fn a_file_target_spools_rather_than_overwriting() {
        // The file is the day's paper. A reprint that erased the previous receipt would destroy
        // the only record of what was actually handed over.
        let path = std::env::temp_dir().join(format!("sahl-print-{}.bin", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let target = PrinterTarget::File(path.clone());

        print(&target, b"first").expect("prints");
        print(&target, b"second").expect("prints again");

        let spooled = std::fs::read(&path).expect("reads");
        assert_eq!(spooled, b"firstsecond");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn an_unreachable_printer_reports_its_address() {
        // Port 1 on the loopback: nothing listens there, and it refuses immediately rather than
        // making the test wait out the timeout.
        let result = print(&PrinterTarget::Network("127.0.0.1:1".to_owned()), b"job");
        assert!(matches!(result, Err(PrintError::Unreachable { .. })));
    }
}
