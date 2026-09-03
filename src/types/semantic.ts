export interface SemanticEntityRef {
  entityId: string | null;
  name: string;
  entityType: string;
  filePath: string;
  startLine: number;
  endLine: number;
}

/** 文件内实体(带父子关系,用于结构树构建) */
export interface SemanticFileEntity extends SemanticEntityRef {
  parentId: string | null;
}

export interface SemanticFileEntitiesResult {
  engineVersion: string;
  filePath: string;
  entities: SemanticFileEntity[];
  truncated: boolean;
}

export interface SemanticFindResult {
  engineVersion: string;
  query: string;
  results: SemanticEntityRef[];
  truncated: boolean;
}

/** callers/refs 分组:目标实体 + 关系项(名称不唯一时可能有多组) */
export interface SemanticRelationGroup {
  entity: SemanticEntityRef;
  related: SemanticEntityRef[];
}

export interface SemanticRelationResult {
  engineVersion: string;
  groups: SemanticRelationGroup[];
  truncated: boolean;
}

export interface SemanticImpactedEntity extends SemanticEntityRef {
  depth: number;
}

export interface SemanticImpactResult {
  engineVersion: string;
  entity: SemanticEntityRef;
  dependencies: SemanticEntityRef[];
  dependents: SemanticEntityRef[];
  affected: SemanticImpactedEntity[];
  tests: SemanticEntityRef[];
  total: number;
  depth: number;
  truncated: boolean;
}

export interface SemanticBlameEntry {
  name: string;
  entityType: string;
  startLine: number;
  endLine: number;
  author: string;
  commit: string;
  date: string;
  summary: string;
}

export interface SemanticFileBlameResult {
  engineVersion: string;
  filePath: string;
  entries: SemanticBlameEntry[];
  truncated: boolean;
}

export interface SemanticEntityLogChange {
  changeType: string;
  structuralChange: boolean | null;
  filePath: string;
  commitSha: string;
  author: string;
  date: string;
  message: string;
}

export interface SemanticEntityLogResult {
  engineVersion: string;
  entity: string;
  entityType: string;
  filePath: string;
  changes: SemanticEntityLogChange[];
  truncated: boolean;
}

/** sem context 单条上下文;content 为源码片段,仅用户显式触发后经 IPC 返回 */
export interface SemanticContextEntry {
  entityId: string;
  name: string;
  entityType: string;
  filePath: string;
  role: string;
  tokens: number;
  content: string;
}

/** 因预算被省略的一组实体(按角色聚合计数) */
export interface SemanticContextOmitted {
  role: string;
  entities: number;
  tests: number;
}

export interface SemanticContextResult {
  engineVersion: string;
  entity: string;
  entityId: string;
  budget: number;
  totalTokens: number;
  truncated: boolean;
  targetOmitted: boolean;
  entries: SemanticContextEntry[];
  omitted: SemanticContextOmitted[];
}

/** 一位提交者的统计(git_project_stats;email 归并,展示名为最近一次使用的名字) */
