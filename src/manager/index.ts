import { createServer, type IncomingMessage } from "node:http";
import { timingSafeEqual } from "node:crypto";

import Docker from "dockerode";
import { z } from "zod";

const provisionSchema = z.object({
  runnerId: z.string().min(1),
  poolId: z.string().min(1),
  name: z.string().regex(/^[a-zA-Z0-9._-]+$/),
  image: z.string().min(1),
  jitConfig: z.string().min(20),
  cpuLimit: z.number().positive().max(64),
  memoryLimitMb: z.number().int().positive().max(262_144),
  network: z.string().regex(/^[a-zA-Z0-9_.-]+$/),
});

const docker = new Docker({
  socketPath: process.env.GRIDOPS_DOCKER_SOCKET ?? "/var/run/docker.sock",
});
const port = Number(process.env.GRIDOPS_MANAGER_PORT ?? 8788);
const host = process.env.GRIDOPS_MANAGER_HOST ?? "127.0.0.1";
function requiredEnvironment(name: string) {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is required by the runner manager.`);
  return value;
}

const managerToken = requiredEnvironment("GRIDOPS_MANAGER_TOKEN");

function authorized(value: string | undefined) {
  if (!value?.startsWith("Bearer ")) return false;
  const supplied = Buffer.from(value.slice(7));
  const expected = Buffer.from(managerToken);
  return supplied.length === expected.length && timingSafeEqual(supplied, expected);
}

function json(body: unknown, status = 200) {
  return {
    status,
    headers: { "Content-Type": "application/json", "Cache-Control": "no-store" },
    body: JSON.stringify(body),
  };
}

async function readJson(request: IncomingMessage) {
  const chunks: Buffer[] = [];
  for await (const chunk of request) chunks.push(Buffer.from(chunk));
  if (chunks.reduce((total, chunk) => total + chunk.length, 0) > 1_000_000) {
    throw new Error("Request body exceeds 1 MB.");
  }
  return JSON.parse(Buffer.concat(chunks).toString("utf8")) as unknown;
}

async function ensureNetwork(name: string) {
  const matches = await docker.listNetworks({ filters: JSON.stringify({ name: [name] }) });
  const existing = matches.find((network) => network.Name === name);
  if (existing) return existing.Id;

  const network = await docker.createNetwork({
    Name: name,
    Driver: "bridge",
    CheckDuplicate: true,
    Internal: false,
    Labels: { "io.gridops.managed": "true" },
  });
  return network.id;
}

async function ensureImage(image: string) {
  try {
    await docker.getImage(image).inspect();
    return;
  } catch (error) {
    const status = (error as { statusCode?: number }).statusCode;
    if (status !== 404) throw error;
  }

  const stream = await docker.pull(image);
  await new Promise<void>((resolve, reject) => {
    docker.modem.followProgress(stream, (error) => error ? reject(error) : resolve());
  });
}

async function provisionRunner(input: z.infer<typeof provisionSchema>) {
  await ensureNetwork(input.network);
  await ensureImage(input.image);
  const existing = await docker.listContainers({
    all: true,
    filters: JSON.stringify({ name: [input.name] }),
  });
  if (existing.some((container) => container.Names?.includes(`/${input.name}`))) {
    return json({ error: "A runner container with this name already exists." }, 409);
  }

  const container = await docker.createContainer({
    name: input.name,
    Image: input.image,
    Cmd: ["/home/runner/run.sh", "--jitconfig", input.jitConfig],
    Labels: {
      "io.gridops.managed": "true",
      "io.gridops.runner-id": input.runnerId,
      "io.gridops.pool-id": input.poolId,
    },
    HostConfig: {
      AutoRemove: false,
      NetworkMode: input.network,
      NanoCpus: Math.floor(input.cpuLimit * 1_000_000_000),
      Memory: input.memoryLimitMb * 1024 * 1024,
      MemorySwap: input.memoryLimitMb * 1024 * 1024,
      PidsLimit: 2048,
      CapDrop: ["ALL"],
      SecurityOpt: ["no-new-privileges:true"],
    },
  });

  await container.start();
  const details = await container.inspect();
  return json({
    id: details.Id,
    name: details.Name.replace(/^\//, ""),
    state: details.State.Status,
    createdAt: details.Created,
  }, 201);
}

async function handle(request: IncomingMessage) {
  const url = new URL(request.url ?? "/", `http://${request.headers.host ?? "localhost"}`);

  if (!authorized(request.headers.authorization)) return json({ error: "Unauthorized" }, 401);

  if (request.method === "GET" && url.pathname === "/v1/health") {
    await docker.ping();
    const version = await docker.version();
    return json({ status: "ok", dockerVersion: version.Version, apiVersion: version.ApiVersion });
  }

  if (request.method === "GET" && url.pathname === "/v1/runners") {
    const containers = await docker.listContainers({
      all: true,
      filters: JSON.stringify({ label: ["io.gridops.managed=true"] }),
    });
    return json({
      runners: containers.map((container) => ({
        id: container.Id,
        names: container.Names,
        image: container.Image,
        state: container.State,
        status: container.Status,
        labels: container.Labels,
        createdAt: new Date(container.Created * 1000).toISOString(),
      })),
    });
  }

  if (request.method === "POST" && url.pathname === "/v1/runners") {
    return provisionRunner(provisionSchema.parse(await readJson(request)));
  }

  const runnerMatch = url.pathname.match(/^\/v1\/runners\/([a-f0-9]{12,64})(?:\/(stop|pause|resume|restart|logs))?$/i);
  if (runnerMatch) {
    const [, id, action] = runnerMatch;
    const container = docker.getContainer(id!);

    if (request.method === "POST" && action === "stop") {
      await container.stop({ t: 30 }).catch((error: unknown) => {
        if (!(error instanceof Error && error.message.includes("304"))) throw error;
      });
      return json({ status: "stopped" });
    }
    if (request.method === "POST" && action === "pause") {
      await container.pause();
      return json({ status: "paused" });
    }
    if (request.method === "POST" && action === "resume") {
      await container.unpause();
      return json({ status: "running" });
    }
    if (request.method === "POST" && action === "restart") {
      await container.restart({ t: 30 });
      return json({ status: "running" });
    }
    if (request.method === "GET" && action === "logs") {
      const logs = await container.logs({ stdout: true, stderr: true, timestamps: true, tail: 500 });
      return {
        status: 200,
        headers: { "Content-Type": "text/plain; charset=utf-8", "Cache-Control": "no-store" },
        body: decodeDockerLogs(Buffer.isBuffer(logs) ? logs : Buffer.from(logs as unknown as Uint8Array)),
      };
    }
    if (request.method === "DELETE" && !action) {
      await container.remove({ force: true, v: true });
      return json({ status: "deleted" });
    }
  }

  return json({ error: "Not found" }, 404);
}

const server = createServer(async (request, response) => {
  try {
    const result = await handle(request);
    response.writeHead(result.status, result.headers);
    response.end(result.body);
  } catch (error) {
    const dockerStatus = (error as { statusCode?: number }).statusCode;
    const status = error instanceof z.ZodError ? 400 : dockerStatus === 404 ? 404 : 500;
    const message = error instanceof Error ? error.message : "Unknown runner manager error";
    response.writeHead(status, { "Content-Type": "application/json", "Cache-Control": "no-store" });
    response.end(JSON.stringify({ error: message }));
  }
});

function decodeDockerLogs(buffer: Buffer) {
  const chunks: Buffer[] = [];
  let offset = 0;
  while (offset + 8 <= buffer.length && (buffer[offset] === 1 || buffer[offset] === 2)) {
    const size = buffer.readUInt32BE(offset + 4);
    const start = offset + 8;
    const end = start + size;
    if (end > buffer.length) break;
    chunks.push(buffer.subarray(start, end));
    offset = end;
  }
  return chunks.length > 0 && offset === buffer.length ? Buffer.concat(chunks) : buffer;
}

server.listen(port, host, () => {
  console.info(`GridOps runner manager listening on ${host}:${port}`);
});
