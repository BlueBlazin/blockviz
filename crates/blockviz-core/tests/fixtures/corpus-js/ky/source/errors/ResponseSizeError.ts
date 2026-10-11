import type {KyRequest} from '../types/request.js';
import {KyError} from './KyError.js';

/**
Error thrown when the response body exceeds `maxResponseSize`. It has a `request` property with the `Request` object and a `maxResponseSize` property with the configured limit in bytes.

@example
```
import ky, {isResponseSizeError} from 'ky';

try {
	await ky('https://example.com/data', {maxResponseSize: 1024}).json();
} catch (error) {
	if (isResponseSizeError(error)) {
		console.log(`Response exceeded ${error.maxResponseSize} bytes`);
	}
}
```
*/
export class ResponseSizeError extends KyError {
	override name = 'ResponseSizeError';
	request: KyRequest;
	maxResponseSize: number;

	constructor(request: Request, maxResponseSize: number) {
		super(`Response body exceeded ${maxResponseSize} bytes: ${request.method} ${request.url}`);
		this.request = request;
		this.maxResponseSize = maxResponseSize;
	}
}
