export class Database {
  constructor() {
    throw new Error(
      "bun:sqlite is unavailable under node; session-control tests use an in-memory repository",
    );
  }
}
