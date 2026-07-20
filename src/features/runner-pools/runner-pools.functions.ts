import { createServerFn } from "@tanstack/react-start";
import { getRequest, setResponseHeaders } from "@tanstack/react-start/server";
import { z } from "zod";

import { createRunnerPoolSchema } from "./schemas";
import {
  controlRunner,
  createRunnerPool,
  deleteRunnerPool,
  getRunnerPoolFormOptions,
  provisionRunner,
  reconcileRunnerPool,
  scaleRunnerPool,
  setRunnerPoolPaused,
} from "./runner-pools.server";
import { getSessionUser } from "~/server/auth/session.server";

function requireUser() {
  const user = getSessionUser(getRequest());
  if (!user) throw new Error("Connect GitHub before managing runner pools.");
  return { id: user.id, login: user.login };
}

function privateResponse() {
  setResponseHeaders(new Headers({ "Cache-Control": "private, no-store", Vary: "Cookie" }));
}

export const getCreateRunnerPoolOptions = createServerFn({ method: "GET" }).handler(
  async () => {
    privateResponse();
    const user = getSessionUser(getRequest());
    if (!user) {
      return {
        authenticated: false as const,
        installations: [],
        repositories: [],
        defaults: null,
      };
    }
    return {
      authenticated: true as const,
      ...getRunnerPoolFormOptions({ id: user.id, login: user.login }),
    };
  },
);

export const createRunnerPoolAction = createServerFn({ method: "POST" })
  .validator(createRunnerPoolSchema)
  .handler(async ({ data }) => {
    privateResponse();
    return createRunnerPool(requireUser(), data);
  });

export const provisionRunnerAction = createServerFn({ method: "POST" })
  .validator(z.object({ poolId: z.string().min(1) }))
  .handler(async ({ data }) => {
    privateResponse();
    return provisionRunner(requireUser(), data.poolId);
  });

export const runnerPoolAction = createServerFn({ method: "POST" })
  .validator(z.discriminatedUnion("action", [
    z.object({ action: z.literal("pause"), poolId: z.string().min(1) }),
    z.object({ action: z.literal("resume"), poolId: z.string().min(1) }),
    z.object({ action: z.literal("reconcile"), poolId: z.string().min(1) }),
    z.object({ action: z.literal("delete"), poolId: z.string().min(1) }),
    z.object({
      action: z.literal("scale"),
      poolId: z.string().min(1),
      desiredCount: z.number().int().min(0).max(100),
    }),
  ]))
  .handler(async ({ data }) => {
    privateResponse();
    const user = requireUser();
    switch (data.action) {
      case "pause": return setRunnerPoolPaused(user, data.poolId, true);
      case "resume": return setRunnerPoolPaused(user, data.poolId, false);
      case "reconcile": return reconcileRunnerPool(user, data.poolId);
      case "delete": return deleteRunnerPool(user, data.poolId);
      case "scale": return scaleRunnerPool(user, data.poolId, data.desiredCount);
    }
  });

export const runnerAction = createServerFn({ method: "POST" })
  .validator(z.object({
    runnerId: z.string().min(1),
    action: z.enum(["stop", "pause", "resume", "restart", "rebuild", "delete"]),
  }))
  .handler(async ({ data }) => {
    privateResponse();
    return controlRunner(requireUser(), data.runnerId, data.action);
  });
