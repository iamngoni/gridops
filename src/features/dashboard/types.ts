export type DashboardOverview = {
  authenticated: boolean;
  configuration: {
    githubOAuth: boolean;
    githubAppControl: boolean;
    webhookVerification: boolean;
    secureStorage: boolean;
    runnerManager: boolean;
    callbackUrl: string;
    webhookUrl: string;
  };
  metrics: {
    runners: number;
    online: number;
    busy: number;
    queuedJobs: number;
    successRate: number | null;
  };
  pools: Array<{
    id: string;
    name: string;
    scope: string;
    desired: number;
    online: number;
    busy: number;
    queue: number;
    mode: string;
    status: string;
  }>;
  runs: Array<{
    id: number;
    repository: string;
    workflow: string;
    branch: string | null;
    status: string;
    conclusion: string | null;
    startedAt: string | null;
    completedAt: string | null;
    htmlUrl: string;
  }>;
  activity: Array<{
    id: string;
    level: string;
    event: string;
    message: string;
    createdAt: string;
  }>;
  installations: number;
};
