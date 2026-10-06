import { Context, Service } from "cordis";
// Side-effect import: augments cordis Context with the `controlPlane` key this service injects.
import "@loams-plugins/plugin-control-plane";

export class DataService extends Service {
  static inject = ["controlPlane"];

  private _cache: Map<string, { data: any; ts: number }> = new Map();
  private _inflight: Map<string, Promise<any>> = new Map();
  private _params: Map<string, unknown> = new Map();
  private _cacheTTL = 5 * 60 * 1000;

  constructor(ctx: Context) {
    super(ctx, "data");
  }

  setParam(name: string, value: unknown) {
    this._params.set(name, value);
    this.ctx.emit("data/param-changed" as any, name, value);
  }

  getParam(name: string) {
    return this._params.get(name);
  }

  getParams() {
    return Object.fromEntries(this._params);
  }

  async fetchWidgetData(widget: any, params?: Record<string, unknown>) {
    const mergedParams = {
      ...this.getParams(),
      ...widget.params,
      ...params,
    };
    const key = this._buildCacheKey(widget, mergedParams);

    const cached = this._cache.get(key);
    if (cached && Date.now() - cached.ts < this._cacheTTL) {
      return cached.data;
    }

    if (this._inflight.has(key)) {
      return this._inflight.get(key);
    }

    const promise = this._executeQuery(widget, mergedParams)
      .then((data) => {
        this._cache.set(key, { data, ts: Date.now() });
        this._inflight.delete(key);
        return data;
      })
      .catch((err) => {
        this._inflight.delete(key);
        throw err;
      });

    this._inflight.set(key, promise);
    return promise;
  }

  invalidate(datasetId?: string) {
    if (datasetId) {
      for (const [key] of this._cache.entries()) {
        if (key.includes(datasetId)) {
          this._cache.delete(key);
        }
      }
    } else {
      this._cache.clear();
    }
  }

  private _bindParams(widget: any, params: Record<string, unknown>) {
    return {
      ...widget,
      params: {
        ...widget.params,
        ...params,
      },
    };
  }

  private _buildCacheKey(widget: any, params: Record<string, unknown>) {
    return JSON.stringify({
      w: widget,
      p: params,
    });
  }

  private async _executeQuery(widget: any, params: Record<string, unknown> = {}) {
    if (typeof this.ctx.controlPlane?.queryData !== "function") {
      return { data: [], rowcount: 0 };
    }
    const bound = this._bindParams(widget, params);
    const res = await this.ctx.controlPlane.queryData(bound);
    if (res?.result?.[0]) {
      return res.result[0];
    }
    return res || { data: [], rowcount: 0 };
  }
}

declare module "cordis" {
  interface Context {
    data: DataService;
  }
}
