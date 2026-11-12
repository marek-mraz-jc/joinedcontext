# sql parity with Bento 1.22.0

Written by `scripts/parity-sql.py record`; each case runs its configs in Bento and in ekiben
against the same fresh databases (postgres and mysql images, sqlite file).

| Case | Result |
|---|---|
| lint/deprecated-sql-conn-field | same |
| lint/driver-clickhouse | deviation (Bento accepted, ekiben refused: see deviation.txt) |
| lint/driver-pgx | same |
| lint/iam-and-secret | same |
| lint/insert-missing-columns | same |
| lint/unknown-field | same |
| sql-output/sqlite-deprecated | identical |
| sql_insert-output/mysql | identical |
| sql_insert-output/postgres | identical |
| sql_insert-output/sqlite | identical |
| sql_raw-input/mysql-binary-protocol | identical |
| sql_raw-input/mysql-parse-time | identical |
| sql_raw-input/mysql-text-protocol | identical |
| sql_raw-input/postgres-args | identical |
| sql_raw-input/postgres-types | identical |
| sql_raw-input/sqlite-args-non-array | both fail |
| sql_raw-input/sqlite-types | identical |
| sql_raw-output/mysql-error | both fail |
| sql_raw-output/postgres-static-query-not-interpolated | identical |
| sql_raw-output/postgres-unsafe-dynamic-query | identical |
| sql_select-input/mysql-where-args | identical |
| sql_select-input/postgres-prefix-columns | identical |
| sql_select-input/postgres-where-args | identical |
| sql_select-input/sqlite-where-args | identical |
