import { Database } from "bun:sqlite";

/**
 * The tiny slice of the sqlite driver sepia needs, as a structural seam.
 * `ClineStore` and `SessionSqlite` consume the interface; bun provides the
 * implementation. This keeps unit tests runnable under node, where `bun:sqlite`
 * does not exist, while the CLI runs entirely on bun as before.
 */
export interface SessionSqlite {
  readonly run: (sql: string) => void;
  readonly get: <T>(sql: string, ...params: Array<string>) => T | null;
  readonly allTables: () => ReadonlyArray<string>;
  readonly insertSession: (
    columns: ReadonlyArray<string>,
    values: ReadonlyArray<string | number | null>,
  ) => void;
  readonly close: () => void;
}

export const openSessionsDb = (dbPath: string, readonly: boolean): SessionSqlite => {
  const sqlite = readonly ? new Database(dbPath, { readonly: true }) : new Database(dbPath);
  return {
    run: (sql) => {
      sqlite.run("PRAGMA busy_timeout = 5000;");
      sqlite.run(sql);
    },
    get: <T>(sql: string, ...params: Array<string>): T | null =>
      sqlite.query<T, Array<string>>(sql).get(...params),
    allTables: () =>
      sqlite
        .query<{ name: string }, []>("select name from sqlite_master where type = 'table'")
        .all()
        .map((table) => table.name),
    insertSession: (columns, values) => {
      sqlite
        .query(
          `INSERT OR REPLACE INTO sessions (${columns.join(", ")}) VALUES (${columns.map(() => "?").join(", ")})`,
        )
        .run(...values);
    },
    close: () => sqlite.close(),
  };
};
