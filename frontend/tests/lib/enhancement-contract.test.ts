import { expect, test } from 'bun:test';
import golden from '../fixtures/meeting-document-v1.json';
import { MeetingDocumentSchema } from '../../src/types/enhancement';

test('TypeScript accepts the same snake_case document fixture as the Rust snapshot',()=>{
  expect<unknown>(MeetingDocumentSchema.parse(golden)).toEqual(golden);
});
test('document contract rejects invented timing and invalid versions',()=>{
  expect(MeetingDocumentSchema.safeParse({...golden,versions:{...golden.versions,revision:-1}}).success).toBe(false);
  expect(MeetingDocumentSchema.safeParse({...golden,transcript:[{...golden.transcript[0],start_ms:10,end_ms:5}]}).success).toBe(false);
});
