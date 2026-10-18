// IOS-8: the iPhone key row (stub, Red).
export interface KeyRowActions {
  sendKeys(keys: string[]): void;
  typeText(text: string): void;
  setTouchMode(mode: string): void;
}

/** Renders the key row into `root`. */
export function renderKeyRow(_root: HTMLElement, _touch: string, _actions: KeyRowActions): void {}
