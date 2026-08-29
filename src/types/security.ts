export interface AuthenticationStatus {
  configured: boolean;
  authenticated: boolean;
  failed_attempts: number;
  remaining_attempts: number;
  locked_out: boolean;
}

export interface Permissions {
  read: boolean;
  write: boolean;
  execute: boolean;
}

export interface ResourceMetadata {
  id: number;
  name: string;
  resource_type: "directory" | "file";
  created_at_ms: number;
  modified_at_ms: number;
  parent: number | null;
  size: number;
  permissions: Permissions;
  locked: boolean;
  owner: string;
}

export interface ResourceInfo {
  path: string;
  metadata: ResourceMetadata;
}

export interface ResourceSecurityInfo {
  resource: ResourceInfo;
  pending_lock_boundaries: string[];
}

export interface ResourceAuthenticationStatus {
  path: string;
  authenticated_boundary_id: number | null;
  remaining_boundaries: number;
}
