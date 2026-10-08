export interface BiConfig {
  baseUrl: string;
  username?: string;
  password?: string;
}

export interface BiDataset {
  id: number;
  table_name: string;
  [key: string]: any;
}
