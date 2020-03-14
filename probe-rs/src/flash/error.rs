use thiserror::Error;

use crate::config::FlashRegion;
use crate::error;

#[derive(Error, Debug)]
pub enum FlashError {
    #[error("The execution of '{name}' failed with code {errorcode}")]
    RoutineCallFailed { name: &'static str, errorcode: u32 },
    #[error("'{0}' is not supported")]
    NotSupported(&'static str),
    #[error("Buffer {n}/{max} does not exist")]
    InvalidBufferNumber { n: usize, max: usize },
    #[error("Something during memory interaction went wrong: {0}")]
    Memory(#[source] error::Error),
    #[error("Something during the interaction with the core went wrong: {0}")]
    Core(#[source] error::Error),
    #[error("{address} is not contained in {region:?}")]
    AddressNotInRegion { address: u32, region: FlashRegion },
    #[error(
        "The RAM contents did not match the expected contents after loading the flash algorithm."
    )]
    FlashAlgorithmNotLoaded,
    #[error(
        "The page write of the page at address {page_address:#08X} failed with error code {error_code}."
    )]
    PageWrite { page_address: u32, error_code: u32 },
    #[error("Overlap in data, address {0:#010x} was already written earlier.")]
    DataOverlap(u32),
    #[error("Address {0:#010x} is not a valid address in the flash area.")]
    InvalidFlashAddress(u32),
    #[error("There is already an other entry for address {0:#010x}")]
    DuplicateDataEntry(u32),
    #[error("Internal error: The sector configuration is expecting page size {sector_page_size}, but the actual  page size is {page_size}.")]
    PageSizeDoesNotMatch {
        sector_page_size: u32,
        page_size: u32,
    },
    #[error("The maximum page count {maximum_page_count} for the sector at address {sector_address:#010x} was exceeded.")]
    MaxPageCountExceeded {
        maximum_page_count: usize,
        sector_address: u32,
    },
    #[error(
        "No flash memory contains the entire requested memory range {start:#08X}..{end:#08X}."
    )]
    NoSuitableFlash { start: u32, end: u32 },
    #[error("Trying to write flash, but no flash loader algorithm is attached.")]
    NoFlashLoaderAlgorithmAttached,
}