/** Shared Rust/browser fixture corpus checks boundary parity, not server authorization. */
import { describe, expect, it } from "vitest";
import fixtures from "./fixtures/fleet-contracts.json";
import {
  backendSummarySchema, fleetErrorSchema, fleetEventSchema, hostActionRequestSchema,
  hostSummarySchema, operationAcceptedSchema, operationStatusSchema, pageSchema,
  placementOutcomeSchema, resourceAmountSchema, submitWorkloadRequestSchema,
  logChunkSchema, logMetadataSchema,
} from "../src/features/fleet/contracts";

const schemas = {
  host: hostSummarySchema, backend: backendSummarySchema, resource: resourceAmountSchema,
  accepted: operationAcceptedSchema, status: operationStatusSchema, outcome: placementOutcomeSchema,
  action: hostActionRequestSchema, submit: submitWorkloadRequestSchema,
  event: fleetEventSchema, error: fleetErrorSchema, page: pageSchema(hostSummarySchema),
  log: logChunkSchema, logMetadata: logMetadataSchema,
};

describe("fleet v1 shared contracts", () => {
  for (const fixture of fixtures) {
    it(fixture.name, () => {
      expect(fixture.schema in schemas).toBe(true);
      const schema = schemas[fixture.schema as keyof typeof schemas];
      const result = schema.safeParse(fixture.body);
      expect(result.success).toBe(fixture.valid);
      if (result.success) expect(schema.safeParse(result.data).success).toBe(true);
    });
  }

  it("bounds pages and keeps stable event identity on cursor renewal", () => {
    const host = hostSummarySchema.parse(fixtures.find((item) => item.schema === "host")?.body);
    expect(schemas.page.safeParse({ items: Array(100).fill(host), nextCursor: null }).success).toBe(true);
    expect(schemas.page.safeParse({ items: Array(101).fill(host), nextCursor: null }).success).toBe(false);
    const event = fleetEventSchema.parse(fixtures.find((item) => item.schema === "event")?.body);
    const replay = fleetEventSchema.parse({ ...event, cursor: "v1.new.signature" });
    expect(replay.id).toBe(event.id);
    expect(replay.cursor).not.toBe(event.cursor);
  });

  it("bounds log content by UTF-8 bytes", () => {
    const log = logChunkSchema.parse(fixtures.find((item) => item.schema === "log")?.body);
    expect(logChunkSchema.safeParse({ ...log, text: "😀".repeat(65536) }).success).toBe(true);
    expect(logChunkSchema.safeParse({ ...log, text: "😀".repeat(65537) }).success).toBe(false);
  });
});
