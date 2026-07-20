import { z } from "zod";

export const createRunnerPoolSchema = z
  .object({
    installationId: z.number().int().positive(),
    repositoryId: z.number().int().positive().nullable(),
    name: z
      .string()
      .trim()
      .min(2)
      .max(48)
      .regex(/^[a-z0-9][a-z0-9-]*[a-z0-9]$/, "Use lowercase letters, numbers, and hyphens."),
    scope: z.enum(["repository", "organization"]),
    mode: z.enum(["ephemeral", "persistent"]),
    labels: z.array(z.string().trim().min(1).max(64)).max(20),
    image: z.string().trim().min(1).max(300),
    desiredCount: z.number().int().min(0).max(50),
    minCount: z.number().int().min(0).max(50),
    maxCount: z.number().int().min(1).max(100),
    autoscalingEnabled: z.boolean().default(true),
    queueScaleFactor: z.number().int().min(1).max(20).default(1),
    idleTimeoutMinutes: z.number().int().min(1).max(1_440).default(5),
    cpuLimit: z.number().positive().max(64),
    memoryLimitMb: z.number().int().min(256).max(262_144),
    runnerGroupId: z.number().int().positive().default(1),
  })
  .superRefine((value, context) => {
    if (value.scope === "repository" && !value.repositoryId) {
      context.addIssue({
        code: "custom",
        path: ["repositoryId"],
        message: "Choose a repository for a repository-scoped pool.",
      });
    }
    if (value.scope === "organization" && value.repositoryId) {
      context.addIssue({
        code: "custom",
        path: ["repositoryId"],
        message: "Organization pools cannot target one repository.",
      });
    }
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
  });

export type CreateRunnerPoolInput = z.infer<typeof createRunnerPoolSchema>;
