export class Database {
  constructor() {
    throw new Error(
      "bun:sqlite is unavailable under node; server tests stub the store or run under bun",
    );
  }
}
