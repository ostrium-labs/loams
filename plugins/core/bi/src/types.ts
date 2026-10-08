export interface ControlPlaneConfig {
  baseUrl: string;
  username?: string;
  password?: string;
}

export interface ControlPlaneDataset {
  id: number;
  table_name: string;
  [key: string]: any;
}
