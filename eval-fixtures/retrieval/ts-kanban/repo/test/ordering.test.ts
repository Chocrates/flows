import { describe, expect, it } from "vitest";
import { compareRank, needsRebalance, rankBetween, spreadRanks } from "../src/shared/ordering";

describe("rank keys", () => {
  it("produces a key between two neighbours", () => {
    const k = rankBetween("a", "c");
    expect(k > "a" && k < "c").toBe(true);
  });

  it("handles open ends", () => {
    expect(rankBetween(null, "m") < "m").toBe(true);
    expect(rankBetween("m", null) > "m").toBe(true);
  });

  it("descends when neighbours are adjacent", () => {
    const k = rankBetween("a", "b");
    expect(k.startsWith("a")).toBe(true);
    expect(k > "a" && k < "b").toBe(true);
  });

  it("rejects out-of-order bounds", () => {
    expect(() => rankBetween("c", "a")).toThrow();
  });

  it("spreads keys in ascending order", () => {
    const ks = spreadRanks(50);
    expect([...ks].sort()).toEqual(ks);
    expect(new Set(ks).size).toBe(50);
  });

  it("flags long keys for rebalance", () => {
    expect(needsRebalance(["a".repeat(30)])).toBe(true);
    expect(needsRebalance(["abc"])).toBe(false);
  });

  it("breaks rank ties by id", () => {
    expect(compareRank({ rank: "m", id: "a" }, { rank: "m", id: "b" })).toBeLessThan(0);
  });
});
