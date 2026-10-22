// RESP-6 Red: on compact widths the toolbar's search is a button that opens a full-width field
// on its own row (the stylesheet shows the button only below 600 points; findings F8).
import { describe, expect, test } from "bun:test";
import { FakeConnectionsApi, startConnections } from "./fakes";
import { entry, flush, profile } from "./helpers";

async function start() {
  const fake = new FakeConnectionsApi();
  fake.entries = [entry(profile("headless", { name: "Build box" }))];
  const { root } = await startConnections(fake);
  const toolbar = root.querySelector<HTMLElement>("header.toolbar")!;
  const toggle = toolbar.querySelector<HTMLButtonElement>("button.search-toggle")!;
  const field = toolbar.querySelector<HTMLInputElement>("input[type=search]")!;
  return { root, toolbar, toggle, field };
}

describe("compact toolbar search", () => {
  test("a Search button controls the search field", async () => {
    const { toggle, field } = await start();
    expect(toggle).not.toBeNull();
    expect(toggle.getAttribute("aria-label")).toBe("Search");
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(toggle.getAttribute("aria-controls")).toBe(field.id);
  });

  test("opening it shows the search row and focuses the field", async () => {
    const { toolbar, toggle, field } = await start();
    toggle.click();
    await flush();
    expect(toolbar.classList.contains("searching")).toBe(true);
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    expect(document.activeElement).toBe(field);
  });

  test("closing it clears the search and hides the row", async () => {
    const { root, toolbar, toggle, field } = await start();
    toggle.click();
    field.value = "zzz";
    field.dispatchEvent(new Event("input", { bubbles: true }));
    await flush();
    expect(root.querySelector(".no-results")).not.toBeNull();
    toggle.click();
    await flush();
    expect(toolbar.classList.contains("searching")).toBe(false);
    expect(field.value).toBe("");
    expect(root.querySelector(".no-results")).toBeNull();
  });

  test("Escape in an empty field closes it", async () => {
    const { toolbar, toggle, field } = await start();
    toggle.click();
    await flush();
    field.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    await flush();
    expect(toolbar.classList.contains("searching")).toBe(false);
    expect(document.activeElement).toBe(toggle);
  });
});
