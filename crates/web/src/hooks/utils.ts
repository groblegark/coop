import { useRef, useState } from "react";

export function useLatest<T>(value: T) {
  const ref = useRef(value);
  ref.current = value;
  return ref;
}

export function useInit(fn: () => void) {
  const ran = useRef(false);
  if (!ran.current) {
    ran.current = true;
    fn();
  }
}

export function useLocalStorage<T>(
  key: string,
  defaultValue: T,
): [T, (value: T | ((prev: T) => T)) => void] {
  const [state, setState] = useState<T>(() => {
    const stored = localStorage.getItem(key);
    if (stored === null) return defaultValue;
    try {
      return JSON.parse(stored) as T;
    } catch {
      return defaultValue;
    }
  });

  const setValue = (value: T | ((prev: T) => T)) => {
    const next = typeof value === "function" ? (value as (prev: T) => T)(state) : value;
    setState(next);
    localStorage.setItem(key, JSON.stringify(next));
  };

  return [state, setValue];
}
