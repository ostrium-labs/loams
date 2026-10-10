import { Context, Service } from "cordis";
import { BiConfig, BiDataset } from "./types.js";

export class BiService extends Service {
  static inject = [];

  private _token: string | null = null;
  private _tokenExpiry: number = 0;
  private _csrfToken: string | null = null;
  private _datasetCache: Map<number, BiDataset> = new Map();
  public config: BiConfig;

  constructor(ctx: Context, config: BiConfig) {
    super(ctx, "bi");
    this.config = config;
  }

  async _ensureAuth(): Promise<string> {
    if (this._token && Date.now() < this._tokenExpiry) {
      return this._token;
    }

    const response = await fetch(`${this.config.baseUrl}/api/v1/security/login`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
      },
      body: JSON.stringify({
        username: this.config.username,
        password: this.config.password,
        provider: "db",
      }),
    });

    if (!response.ok) {
      throw new Error(`Auth failed: ${response.statusText}`);
    }

    const data = (await response.json()) as any;
    this._token = data.access_token;
    this._tokenExpiry = Date.now() + 55 * 60 * 1000;
    return this._token!;
  }

  async _getCsrfToken(): Promise<string> {
    const token = await this._ensureAuth();
    const response = await fetch(`${this.config.baseUrl}/api/v1/security/csrf_token/`, {
      method: "GET",
      headers: {
        Authorization: `Bearer ${token}`,
      },
    });

    if (!response.ok) {
      throw new Error(`CSRF failed: ${response.statusText}`);
    }

    const data = (await response.json()) as any;
    this._csrfToken = data.result;
    return this._csrfToken!;
  }

  async _fetch(path: string, init?: RequestInit): Promise<Response> {
    const token = await this._ensureAuth();
    const headers = new Headers(init?.headers);
    headers.set("Authorization", `Bearer ${token}`);

    if (init?.method && init.method !== "GET" && init.method !== "HEAD") {
      if (!this._csrfToken) {
        await this._getCsrfToken();
      }
      if (this._csrfToken) {
        headers.set("X-CSRFToken", this._csrfToken);
      }
    }

    const response = await fetch(`${this.config.baseUrl}${path}`, {
      ...init,
      headers,
    });

    if (!response.ok) {
      throw new Error(`Request failed: ${response.status} ${response.statusText}`);
    }

    return response;
  }

  async listDatasets(): Promise<any> {
    const response = await this._fetch("/api/v1/dataset/?q=(page_size:100)");
    return response.json();
  }

  async describeDataset(id: number): Promise<BiDataset> {
    if (this._datasetCache.has(id)) {
      return this._datasetCache.get(id)!;
    }

    const response = await this._fetch(`/api/v1/dataset/${id}`);
    const data = (await response.json()) as any;
    const dataset = data.result;
    this._datasetCache.set(id, dataset);
    return dataset;
  }

  async queryData(
    datasetIdOrPayload: any,
    columns?: string[],
    filters?: any[],
    orderby?: any[],
    rowLimit?: number,
  ): Promise<any> {
    let datasetId = datasetIdOrPayload;
    let queryColumns = columns || [];
    let queryFilters = filters;
    let queryLimit = rowLimit;

    if (typeof datasetIdOrPayload === "object" && datasetIdOrPayload !== null) {
      datasetId = datasetIdOrPayload.datasetId || datasetIdOrPayload.data?.datasetId || 1;
      queryColumns = datasetIdOrPayload.columns || queryColumns;
      if (datasetIdOrPayload.params) {
        queryFilters = Object.entries(datasetIdOrPayload.params)
          .filter(([_, v]) => v !== undefined && v !== null && v !== "")
          .map(([col, val]) => ({ col, op: "==", val }));
      }
    }

    const payload = {
      datasource: { id: datasetId, type: "table" },
      params: typeof datasetIdOrPayload === "object" ? datasetIdOrPayload?.params : undefined,
      queries: [
        {
          columns: queryColumns,
          filters: queryFilters,
          orderby,
          row_limit: queryLimit,
        },
      ],
    };

    const response = await this._fetch("/api/v1/chart/data", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
      },
      body: JSON.stringify(payload),
    });

    const res = (await response.json()) as { result?: any[] } & Record<string, unknown>;
    return res?.result?.[0] || res;
  }

  async createGuestToken(dashboardUuid: string, rlsRules?: any[]): Promise<string> {
    const payload = {
      user: {
        username: "guest",
        first_name: "Guest",
        last_name: "User",
      },
      resources: [{ type: "dashboard", id: dashboardUuid }],
      rls: rlsRules || [],
    };

    const response = await this._fetch("/api/v1/security/guest_token/", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
      },
      body: JSON.stringify(payload),
    });

    const data = (await response.json()) as any;
    return data.token;
  }

  clearCache() {
    this._datasetCache.clear();
  }
}

declare module "cordis" {
  interface Context {
    bi: BiService;
  }
}
