import { z } from "zod";

const configurationShape = {
  name: z
    .string()
    .trim()
    .min(2)
    .max(48)
    .regex(/^[a-z0-9][a-z0-9-]*[a-z0-9]$/, "Use lowercase letters, numbers, and hyphens."),
  mode: z.enum(["ephemeral", "persistent"]),
  provider: z.enum(["docker", "tart"]),
  providers: z.array(z.enum(["docker", "tart"])).min(1).max(2),
  labels: z.array(z.string().trim().min(1).max(64).refine(
    (label) => !["self-hosted", "linux", "macos", "windows", "x64", "arm64", "arm", "x86"].includes(label.toLowerCase()),
    "System labels are assigned automatically by the selected provider.",
  )).max(20),
  image: z.string().trim().min(1).max(300),
  dockerImage: z.string().trim().min(1).max(300),
  tartImage: z.string().trim().min(1).max(300),
  desiredCount: z.number().int().min(0).max(100),
  minCount: z.number().int().min(0).max(100),
  maxCount: z.number().int().min(1).max(100),
  autoscalingEnabled: z.boolean().default(true),
  queueScaleFactor: z.number().int().min(1).max(20).default(1),
  idleTimeoutMinutes: z.number().int().min(1).max(1_440).default(5),
  cpuLimit: z.number().min(0.25).max(64),
  memoryLimitMb: z.number().int().min(256).max(262_144),
  runnerGroupId: z.number().int().positive().default(1),
} as const;

const repositoryIds = z.array(z.number().int().positive()).max(1_000);

function validateCapacity(
  value: { desiredCount: number; minCount: number; maxCount: number },
  context: z.RefinementCtx,
) {
  if (value.minCount > value.maxCount) {
    context.addIssue({
      code: "custom",
      path: ["minCount"],
      message: "Minimum capacity cannot exceed maximum capacity.",
    });
  }
  if (value.desiredCount > value.maxCount) {
    context.addIssue({
      code: "custom",
      path: ["desiredCount"],
      message: "Desired capacity cannot exceed maximum capacity.",
    });
  }
  if (value.desiredCount < value.minCount) {
    context.addIssue({
      code: "custom",
      path: ["desiredCount"],
      message: "Desired capacity cannot be below minimum capacity.",
    });
  }
}

function validateProvider(
  value: { provider: "docker" | "tart"; providers: Array<"docker" | "tart">; mode: "ephemeral" | "persistent"; cpuLimit: number; memoryLimitMb: number },
  context: z.RefinementCtx,
) {
  if (new Set(value.providers).size !== value.providers.length || value.providers[0] !== value.provider) {
    context.addIssue({ code: "custom", path: ["providers"], message: "Choose unique providers with the primary provider first." });
  }
  const includesTart = value.providers.includes("tart");
  if (includesTart && value.mode !== "ephemeral") {
    context.addIssue({ code: "custom", path: ["mode"], message: "Pools that include Tart must be ephemeral." });
  }
  if (includesTart && !Number.isInteger(value.cpuLimit)) {
    context.addIssue({ code: "custom", path: ["cpuLimit"], message: "Tart runners require whole CPU cores." });
  }
  if (includesTart && value.memoryLimitMb < 2_048) {
    context.addIssue({ code: "custom", path: ["memoryLimitMb"], message: "Tart runners require at least 2048 MB of memory." });
  }
}

export const updateRunnerPoolSchema = z
  .object({ repositoryIds: repositoryIds.optional(), ...configurationShape })
  .superRefine((value, context) => {
    if (value.repositoryIds?.length === 0) {
      context.addIssue({
        code: "custom",
        path: ["repositoryIds"],
        message: "Choose at least one repository for the pool.",
      });
    }
    if (value.repositoryIds && value.repositoryIds.length > value.maxCount) {
      context.addIssue({
        code: "custom",
        path: ["repositoryIds"],
        message: "Repository count cannot exceed maximum runner capacity.",
      });
    }
    validateCapacity(value, context);
    validateProvider(value, context);
  });

export const createRunnerPoolSchema = z
  .object({
    installationId: z.number().int().positive(),
    repositoryIds: repositoryIds.default([]),
    scope: z.enum(["repository", "organization"]),
    ...configurationShape,
  })
  .superRefine((value, context) => {
    if (value.scope === "repository" && value.repositoryIds.length === 0) {
      context.addIssue({
        code: "custom",
        path: ["repositoryIds"],
        message: "Choose at least one repository for a repository-scoped pool.",
      });
    }
    if (value.scope === "organization" && value.repositoryIds.length > 0) {
      context.addIssue({
        code: "custom",
        path: ["repositoryIds"],
        message: "Organization pools use runner-group repository access.",
      });
    }
    if (value.repositoryIds.length > value.maxCount) {
      context.addIssue({
        code: "custom",
        path: ["repositoryIds"],
        message: "Repository count cannot exceed maximum runner capacity.",
      });
    }
    validateCapacity(value, context);
    validateProvider(value, context);
  });

export function parseCreateRunnerPoolInput(input: unknown) {
  const result = createRunnerPoolSchema.safeParse(input);
  if (!result.success) {
    throw new Error(result.error.issues[0]?.message ?? "Runner pool configuration is invalid.");
  }
  return result.data;
}

export function parseUpdateRunnerPoolInput(input: unknown) {
  const result = updateRunnerPoolSchema.safeParse(input);
  if (!result.success) {
    throw new Error(result.error.issues[0]?.message ?? "Runner pool configuration is invalid.");
  }
  return result.data;
}

export type CreateRunnerPoolInput = z.infer<typeof createRunnerPoolSchema>;
export type UpdateRunnerPoolInput = z.infer<typeof updateRunnerPoolSchema>;
