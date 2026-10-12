import { describe, expect, it } from 'vitest';
import { splitTitle } from './renderer';

describe('titles', () => {
  it('split at the space nearest the middle', () => {
    expect(splitTitle('External packages')).toEqual(['External', 'packages']);
    expect(splitTitle('N_layer, n_head and n_embd are determined')).toEqual(['N_layer, n_head and', 'n_embd are determined']);
    expect(splitTitle('single')).toEqual(['single', '']);
  });
});
