import { describe, expect, it } from "vitest";

import { evaluatePage, sanitizeUrl } from "./privacy";

describe("browser privacy gate", () => {
  it("removes sensitive URL values and fragments", () => {
    const result = sanitizeUrl(
      "https://example.test/article?topic=rust&token=secret&email=a%40example.test#magic",
    );

    expect(result?.url).toBe("https://example.test/article?topic=rust");
    expect(result?.removedQueryKeys).toEqual(["token", "email"]);
  });

  it("allows article content only after positive host and page checks", () => {
    const allowed = evaluatePage(
      "https://example.test/article",
      ["example.test"],
      [],
      { hasPasswordInput: false, hasEmailInput: false, hasContenteditable: false, noindex: false },
    );
    const privateForm = evaluatePage(
      "https://example.test/article",
      ["example.test"],
      [],
      { hasPasswordInput: true, hasEmailInput: false, hasContenteditable: false, noindex: false },
    );
    const newsletterForm = evaluatePage(
      "https://example.test/article",
      ["example.test"],
      [],
      { hasPasswordInput: false, hasEmailInput: true, hasContenteditable: false, noindex: false },
    );

    expect(allowed).toMatchObject({ observe: true, contentAllowed: true });
    expect(privateForm).toMatchObject({ observe: true, contentAllowed: false });
    expect(newsletterForm).toMatchObject({ observe: true, contentAllowed: true });
  });

  it("keeps sensitive paths, editable pages, and noindex behind metadata-only capture", () => {
    expect(evaluatePage("https://example.test/settings", ["example.test"], [], {}))
      .toMatchObject({ observe: true, contentAllowed: false });
    expect(evaluatePage("https://example.test/article", ["example.test"], [], { hasContenteditable: true }))
      .toMatchObject({ observe: true, contentAllowed: false });
    expect(evaluatePage("https://example.test/article", ["example.test"], [], { noindex: true }))
      .toMatchObject({ observe: true, contentAllowed: false });
  });

  it("rejects excluded and local hosts before metadata capture", () => {
    expect(
      evaluatePage("https://private.example.test/inbox", [], ["private.example.test"], {}),
    ).toMatchObject({ observe: false });
    expect(evaluatePage("http://localhost:3000/", [], [], {})).toMatchObject({ observe: false });
    expect(evaluatePage("http://nas/", [], [], {})).toMatchObject({ observe: false });
    expect(evaluatePage("http://[::1]/", [], [], {})).toMatchObject({ observe: false });
    expect(evaluatePage("http://[fc00::1]/", [], [], {})).toMatchObject({ observe: false });
    expect(evaluatePage("http://[::ffff:127.0.0.1]/", [], [], {})).toMatchObject({ observe: false });
  });
});
