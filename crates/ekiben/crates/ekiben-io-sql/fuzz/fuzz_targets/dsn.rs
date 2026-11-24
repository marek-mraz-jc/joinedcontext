//! Input: a driver byte (postgres, mysql, sqlite by its value mod 3), then the DSN.
#![no_main]

use ekiben_io_sql::conn_fields::{parse_dsn, Driver};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&which, dsn)) = data.split_first() else {
        return;
    };
    let driver = [Driver::Postgres, Driver::MySql, Driver::Sqlite][usize::from(which % 3)];
    let root = std::env::temp_dir().join("ekiben-io-sql-fuzz");
    let _ = parse_dsn(driver, &String::from_utf8_lossy(dsn), Some(&root));
});
