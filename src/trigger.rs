use pgrx::prelude::*;
use pgrx::spi;

/// Dummy trigger function for pgrx SQL generation
/// The actual trigger logic is implemented in SQL via create_trigger_handler()
/// This function should never be called since triggers are created in SQL
#[allow(dead_code)]
pub fn tview_trigger<'a>(_trigger: &'a PgTrigger<'a>) -> Result<
    Option<PgHeapTuple<'a, AllocatedByPostgres>>,
    spi::Error,
> {
    // This function should never be called since triggers are created in SQL
    // If it is called, return an error
    Err(spi::Error::InvalidPosition)
}