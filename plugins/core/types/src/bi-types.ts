export interface BiColumn {
  column_name: string;
  type: string;
  is_dttm: boolean;
  verbose_name?: string;
  filterable: boolean;
  groupby: boolean;
}

export interface BiMetric {
  metric_name: string;
  expression: string;
  verbose_name?: string;
}

export interface BiDataset {
  id: number;
  table_name: string;
  schema: string;
  database: {
    id: number;
    database_name: string;
  };
  columns: BiColumn[];
  metrics: BiMetric[];
  description?: string;
}

export interface BiQueryResult {
  data: Record<string, unknown>[];
  colnames: string[];
  coltypes: number[];
  rowcount: number;
}
