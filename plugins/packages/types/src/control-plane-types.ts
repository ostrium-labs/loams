export interface SupersetColumn {
  column_name: string;
  type: string;
  is_dttm: boolean;
  verbose_name?: string;
  filterable: boolean;
  groupby: boolean;
}

export interface SupersetMetric {
  metric_name: string;
  expression: string;
  verbose_name?: string;
}

export interface ControlPlaneDataset {
  id: number;
  table_name: string;
  schema: string;
  database: {
    id: number;
    database_name: string;
  };
  columns: SupersetColumn[];
  metrics: SupersetMetric[];
  description?: string;
}

export interface SupersetQueryResult {
  data: Record<string, unknown>[];
  colnames: string[];
  coltypes: number[];
  rowcount: number;
}
