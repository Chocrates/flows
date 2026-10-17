/**
 * Thin HTTP client for the board API. Transient failures (network errors,
 * 502/503/504, 429) are retried with exponential backoff; everything else is
 * surfaced to the caller as an ApiError.
 */
import type { ApiErrorBody, BoardSnapshot, CardRecord, ColumnRecord, Id } from "../shared/types";

export class ApiError extends Error {
  constructor(public status: number, public body: ApiErrorBody | null) {
    super(body?.message ?? `HTTP ${status}`);
  }
  get isConflict(): boolean {
    return this.status === 409;
  }
}

export interface RetryPolicy {
  attempts: number;
  baseDelayMs: number;
  maxDelayMs: number;
}

const DEFAULT_RETRY: RetryPolicy = { attempts: 5, baseDelayMs: 250, maxDelayMs: 8000 };
const RETRYABLE = new Set([429, 502, 503, 504]);

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

/** Delay before attempt `n` (0-based). Deterministic: no jitter yet. */
export function backoffDelay(n: number, policy: RetryPolicy, retryAfter?: number): number {
  if (retryAfter !== undefined) return Math.min(retryAfter * 1000, policy.maxDelayMs);
  return Math.min(policy.baseDelayMs * 2 ** n, policy.maxDelayMs);
}

export class BoardApi {
  constructor(
    private baseUrl: string,
    private token: () => string | null,
    readonly clientId: string,
    private retry: RetryPolicy = DEFAULT_RETRY,
  ) {}

  private async request<T>(method: string, path: string, body?: unknown): Promise<T> {
    let lastErr: unknown;
    for (let attempt = 0; attempt < this.retry.attempts; attempt++) {
      let res: Response;
      try {
        res = await fetch(this.baseUrl + path, {
          method,
          headers: {
            "content-type": "application/json",
            "x-client-id": this.clientId,
            ...(this.token() ? { authorization: `Bearer ${this.token()}` } : {}),
          },
          body: body === undefined ? undefined : JSON.stringify(body),
        });
      } catch (err) {
        lastErr = err;
        await sleep(backoffDelay(attempt, this.retry));
        continue;
      }
      if (res.ok) return (res.status === 204 ? undefined : await res.json()) as T;
      if (RETRYABLE.has(res.status) && attempt < this.retry.attempts - 1) {
        const ra = res.headers.get("retry-after");
        await sleep(backoffDelay(attempt, this.retry, ra ? Number(ra) : undefined));
        continue;
      }
      const errBody = (await res.json().catch(() => null)) as ApiErrorBody | null;
      throw new ApiError(res.status, errBody);
    }
    throw lastErr instanceof Error ? lastErr : new Error("request failed");
  }

  loadBoard(boardId: Id): Promise<BoardSnapshot> {
    return this.request("GET", `/boards/${boardId}`);
  }

  createCard(card: Omit<CardRecord, "version" | "updatedAt" | "updatedBy">): Promise<CardRecord> {
    return this.request("POST", `/boards/${card.boardId}/cards`, card);
  }

  updateCard(cardId: Id, baseVersion: number, patch: Partial<CardRecord>): Promise<CardRecord> {
    return this.request("PATCH", `/cards/${cardId}`, { baseVersion, patch });
  }

  moveCard(cardId: Id, baseVersion: number, columnId: Id, rank: string): Promise<CardRecord> {
    return this.request("POST", `/cards/${cardId}/move`, { baseVersion, columnId, rank });
  }

  deleteCard(cardId: Id): Promise<void> {
    return this.request("DELETE", `/cards/${cardId}`);
  }

  renameColumn(columnId: Id, name: string): Promise<ColumnRecord> {
    return this.request("PATCH", `/columns/${columnId}`, { name });
  }
}
