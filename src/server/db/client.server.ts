import "@tanstack/react-start/server-only";

import { mkdirSync } from "node:fs";
import { dirname, resolve } from "node:path";

import BetterSqlite3 from "better-sqlite3";
import { drizzle } from "drizzle-orm/better-sqlite3";
import { migrate } from "drizzle-orm/better-sqlite3/migrator";

import { getConfig } from "../config.server";
import * as schema from "./schema";

type Database = ReturnType<typeof drizzle<typeof schema>>;

let database: Database | undefined;
let sqlite: BetterSqlite3.Database | undefined;

export function getSqlite() {
  if (!sqlite) {
    const databasePath = resolve(getConfig().databasePath);
    mkdirSync(dirname(databasePath), { recursive: true });
    sqlite = new BetterSqlite3(databasePath);
    sqlite.pragma("journal_mode = WAL");
    sqlite.pragma("foreign_keys = ON");
    sqlite.pragma("busy_timeout = 5000");
    sqlite.pragma("synchronous = NORMAL");
  }

  return sqlite;
}

export function getDb() {
  if (!database) {
    database = drizzle(getSqlite(), { schema });
  }

  return database;
}

export function migrateDatabase() {
  migrate(getDb(), { migrationsFolder: resolve("drizzle") });
}
