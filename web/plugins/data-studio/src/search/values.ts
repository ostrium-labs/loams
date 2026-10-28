// Top values of keyword fields for the query bar's suggestions: a terms aggregation per
// (field, prefix), cached for the page's life. While a prefix is being fetched, the
// shorter prefix's values that still match stand in.

import { useCallback, useEffect, useMemo, useReducer, useRef } from 'react';
import { bucketsOf, type DataClient } from '../client.js';
import type { FieldInfo } from './kql.js';

export type ValueList = { value: string; count?: number }[];

const key = (field: string, prefix: string) => `${field}\u0000${prefix}`;

/** Whether a field's values can be listed (a fast keyword-like field). */
export function listable(f: FieldInfo | undefined): boolean {
  return !!f && f.fast && (f.type === 'keyword' || f.type === 'uuid');
}

export function useFieldValues(client: DataClient, ns: string, coll: string, fields: FieldInfo[]) {
  const cache = useRef(new Map<string, ValueList>());
  const pending = useRef(new Set<string>());
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const [version, bump] = useReducer((x: number) => x + 1, 0);

  // A new collection starts a new cache.
  // oxlint-disable-next-line react/exhaustive-deps -- reset on ns/coll
  useEffect(() => {
    cache.current = new Map();
    pending.current = new Set();
    bump();
  }, [ns, coll]);
  useEffect(() => () => clearTimeout(timer.current), []);

  // oxlint-disable-next-line react/exhaustive-deps -- `version` makes a new getter after a fetch
  const get = useMemo(
    () =>
      (field: string, prefix: string): ValueList | undefined => {
        const hit = cache.current.get(key(field, prefix));
        if (hit) return hit;
        for (let n = prefix.length - 1; n >= 0; n--) {
          const base = cache.current.get(key(field, prefix.slice(0, n)));
          if (base) return base.filter((v) => v.value.startsWith(prefix));
        }
        return undefined;
      },
    [version],
  );

  const request = useCallback(
    (field: string, prefix: string) => {
      const info = fields.find((f) => f.name === field);
      if (!listable(info)) return;
      const k = key(field, prefix);
      if (cache.current.has(k) || pending.current.has(k)) return;
      clearTimeout(timer.current);
      timer.current = setTimeout(
        () => {
          pending.current.add(k);
          client
            .aggregate(
              ns,
              coll,
              { v: { terms: { field, size: 10 } } },
              prefix ? { prefix: { field, value: prefix } } : undefined,
            )
            .then(
              (aggs) => {
                cache.current.set(
                  k,
                  bucketsOf(aggs.v).map((b) => ({ value: b.key, count: b.count })),
                );
              },
              () => cache.current.set(k, []),
            )
            .finally(() => {
              pending.current.delete(k);
              bump();
            });
        },
        prefix ? 150 : 0,
      );
    },
    [client, ns, coll, fields],
  );

  return { get, request };
}
