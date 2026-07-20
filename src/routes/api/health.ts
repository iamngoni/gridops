import { createFileRoute } from "@tanstack/react-router";

import { getConfigurationState } from "~/server/config.server";
import { getSqlite, migrateDatabase } from "~/server/db/client.server";

export const Route = createFileRoute("/api/health")({
  server: {
    handlers: {
      GET: async () => {
        try {
          migrateDatabase();
          getSqlite().prepare("SELECT 1").get();
          return Response.json(
            {
              status: "ok",
              database: "ok",
              configuration: getConfigurationState(),
              version: "0.1.0",
            },
            { headers: { "Cache-Control": "no-store" } },
          );
        } catch (error) {
          return Response.json(
            {
              status: "error",
              database: "error",
              error: error instanceof Error ? error.message : "Unknown health check error",
            },
            { status: 503, headers: { "Cache-Control": "no-store" } },
          );
        }
      },
    },
  },
});
