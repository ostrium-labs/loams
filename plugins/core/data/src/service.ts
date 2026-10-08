import { Context, Service } from "cordis";
// Side-effect import: augments cordis Context with the `bi` key this service injects.
import "@loams-core/bi";
import { EMPTY_RESULT, type DataProvider, type QueryResult } from "./provider.js";

export class DataService extends Service {
  static inject = ["bi"];

  private _cache: Map<string, { data: any; ts: number }> = new Map();
  private _inflight: Map<string, Promise<any>> = new Map();
  private _params: Map<string, unknown> = new Map();
  private _cacheTTL = 5 * 60 * 1000;
  /** Keyed on `widget.data.source`; the BI backend is the built-in fallback. */
  private _providers: Map<string, DataProvider> = new Map();

  constructor(ctx: Context) {
    super(ctx, "data");
  }

  /**
   * Registers a data source, returning the unregister function.
   *
   * Re-registering a source replaces it, so a plugin that reloads with a new
   * configuration does not stack two providers on one key and leave the
   * previous one's connection open.
   */
  registerProvider(provider: DataProvider): () => void {
    this._providers.set(provider.source, provider);
    return () => {
      if (this._providers.get(provider.source) === provider) {
        this._providers.delete(provider.source);
      }
    };
  }

  /** The provider a widget resolves to, or `undefined` for the the BI backend path. */
  providerFor(widget: any): DataProvider | undefined {
    const source = widget?.data?.source;
    return typeof source === "string" ? this._providers.get(source) : undefined;
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
    const provider = this.providerFor(widget);

    // A live source is read fresh every time. Caching it for five minutes would
    // mean a tile labelled "live" shows whatever was true five minutes ago, and
    // sharing one in-flight promise across two widgets would hand the second one
    // a snapshot taken before its own filter existed.
    if (provider?.live) {
      return provider.query(widget, mergedParams);
    }

    const key = this._buildCacheKey(widget, mergedParams);

    const cached = this._cache.get(key);
    if (cached && Date.now() - cached.ts < this._cacheTTL) {
      return cached.data;
    }

    if (this._inflight.has(key)) {
      return this._inflight.get(key);
    }

    const promise = this._executeQuery(widget, mergedParams, provider)
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

  private async _executeQuery(
    widget: any,
    params: Record<string, unknown> = {},
    provider?: DataProvider,
  ): Promise<QueryResult> {
    if (provider) {
      return provider.query(this._bindParams(widget, params), params);
    }
    if (typeof this.ctx.bi?.queryData !== "function") {
      return EMPTY_RESULT;
    }
    const bound = this._bindParams(widget, params);
    const res = await this.ctx.bi.queryData(bound);
    if (res?.result?.[0]) {
      return res.result[0];
    }
    return res || EMPTY_RESULT;
  }
}

declare module "cordis" {
  interface Context {
    data: DataService;
  }
}
