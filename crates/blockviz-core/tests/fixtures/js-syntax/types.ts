/// <reference types="node" />
import type { Readable } from 'node:stream';
import fs = require('fs');
export type { Readable };

export interface Shape { readonly kind: 'circle' | 'square'; area(): number; [key: string]: unknown; new (x: number): Shape; (y: number): void }
export type Point = { x: number; y: number };
export type Mapped<T> = { [K in keyof T as `get${Capitalize<string & K>}`]: () => T[K] };
type Unwrap<T> = T extends Promise<infer U> ? U : T;
declare const brand: unique symbol;
export const enum Direction { Up = 1, Down, Left = 'LEFT' }
export enum Color { Red, Green, Blue }
declare module 'express' { interface Request { user?: string } }
declare global { interface Window { app: unknown } }
export namespace Geometry { export const unit = 1; export function scale(n: number) { return n * unit; } }

export abstract class Base<const T extends object = {}> {
  protected abstract name: string;
  declare readonly tag: string;
  definite!: number;
  constructor(private readonly store: Map<string, T>, public id: string) {}
  abstract describe(): string;
  override toString(): string { return this.describe(); }
}

export function overload(x: string): string;
export function overload(x: number): number;
export function overload(x: any) { return x; }

export function isPoint(v: unknown): v is Point { return typeof v === 'object' && v !== null && 'x' in v; }
export function assertPoint(v: unknown): asserts v is Point { if (!isPoint(v)) throw new TypeError('not a point'); }

const conf = { port: 3000, host: 'localhost' } as const satisfies Record<string, string | number>;
const el = document.getElementById('app')!;
export function first<T>(xs: readonly T[]): T | undefined { return xs[0]; }
function log(target: any, key: string) {}
function Entity(name: string) { return (cls: Function) => cls; }

@Entity('user')
export class User extends Base<{ name: string }> {
  @log protected name = 'user';
  describe() { return `${this.name}:${this.id}`; }
}
export = conf;
