import { describe, expect, expectTypeOf, it } from "vitest";
import {
  AI_API_TYPES,
  isChatThinkingLevel,
  isKnownAiApiType,
  type AiApiType,
  type KnownAiApiType,
} from "@/lib/ai-config";
import { draftModel, type ProviderDraft } from "@/components/settings/ai-provider-draft";

describe("isKnownAiApiType", () => {
  it("accepts exactly the four implemented wire APIs", () => {
    expect(AI_API_TYPES).toEqual([
      "openai-completions",
      "openai-responses",
      "anthropic-messages",
      "google-generative-ai",
    ]);
    for (const api of AI_API_TYPES) expect(isKnownAiApiType(api)).toBe(true);
  });

  it("keeps unknown external values typed but unclassified", () => {
    const external = "future-wire" as AiApiType;
    expect(isKnownAiApiType(external)).toBe(false);
    expectTypeOf<AiApiType>().toMatchTypeOf<string>();
    expectTypeOf<KnownAiApiType>().toEqualTypeOf<(typeof AI_API_TYPES)[number]>();
  });
});

describe("draftModel api round-trip", () => {
  it("preserves model-level api and falls back to empty for inheritance", () => {
    const override = draftModel({
      id: "claude-x",
      name: "Claude X",
      reasoning: true,
      input: ["text"],
      contextWindow: 200000,
      maxTokens: 64000,
      api: "anthropic-messages",
    });
    expect(override.api).toBe("anthropic-messages");

    const inherited = draftModel({
      id: "gpt-y",
      name: "",
      reasoning: false,
      input: ["text"],
      contextWindow: 0,
      maxTokens: 0,
    });
    expect(inherited.api).toBe("");
  });

  it("builds a provider draft carrying provider api and model drafts", () => {
    const draft: ProviderDraft = {
      key: "k1",
      id: "anthropic",
      name: "Anthropic",
      api: "anthropic-messages",
      baseUrl: "https://api.anthropic.com",
      apiKey: "",
      models: [
        draftModel({
          id: "claude-x",
          name: "",
          reasoning: true,
          input: ["text"],
          contextWindow: 0,
          maxTokens: 0,
          api: "openai-completions",
        }),
      ],
    };
    expect(draft.api).toBe("anthropic-messages");
    expect(draft.models[0].api).toBe("openai-completions");
  });

  it("keeps the thinking-level guard intact", () => {
    expect(isChatThinkingLevel("high")).toBe(true);
    expect(isChatThinkingLevel("ultra")).toBe(false);
  });
});
