-- cursor degraded store — chat 99999999-9999-4999-8999-999999999999 (meta row is plain JSON, not hex)
-- rebuild: sqlite3 store.db < store.sql  (or rusqlite execute_batch)
PRAGMA user_version = 1;
CREATE TABLE blobs (id TEXT PRIMARY KEY, data BLOB);
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);
INSERT INTO blobs (id, data) VALUES ('49881dbdd44c41f4e36b79b211a3807209fec9e566ca69cb7481c1dee22bf942', X'7b22726f6c65223a2275736572222c22636f6e74656e74223a5b7b2274797065223a2274657874222c2274657874223a226c6f6f73652070726f6d7074227d5d7d');
INSERT INTO blobs (id, data) VALUES ('1fe2fe2d68b50d2f476a50fe8de333730a6f9a15a15ddaf3e3e88e4b8cae14d0', X'0a2049881dbdd44c41f4e36b79b211a3807209fec9e566ca69cb7481c1dee22bf9424a1966696c653a2f2f2f686f6d652f64656d6f2f736372617463685001b20103636c69');
INSERT INTO meta (key, value) VALUES ('0', '{"agentId":"99999999-9999-4999-8999-999999999999","latestRootBlobId":"1fe2fe2d68b50d2f476a50fe8de333730a6f9a15a15ddaf3e3e88e4b8cae14d0","name":"Plain Meta","mode":"agent","createdAt":1700000700000,"lastUsedModel":"composer-1"}');
