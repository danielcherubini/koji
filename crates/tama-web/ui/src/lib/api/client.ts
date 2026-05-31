function getCsrfToken(): string | null {
	const cookies = document.cookie.split(';');
	for (const cookie of cookies) {
		const [name, ...rest] = cookie.trim().split('=');
		if (name === 'tama_csrf_token') {
			return rest.join('=');
		}
	}
	return localStorage.getItem('_tama_csrf_token');
}

function storeCsrfToken(token: string): void {
	localStorage.setItem('_tama_csrf_token', token);
}

async function apiFetch(path: string, options: RequestInit = {}): Promise<Response> {
	const url = `/tama/v1${path}`;
	const csrfToken = getCsrfToken();

	const headers: Record<string, string> = {
		...((options.headers as Record<string, string>) || {})
	};

	if (csrfToken) {
		headers['X-CSRF-Token'] = csrfToken;
	}

	const response = await fetch(url, {
		...options,
		headers,
		credentials: 'same-origin'
	});

	const newCsrf = response.headers.get('X-CSRF-Token');
	if (newCsrf) {
		storeCsrfToken(newCsrf);
	}

	return response;
}

const api = {
	get: (path: string, options?: RequestInit) =>
		apiFetch(path, { ...options, method: 'GET' }),
	post: (path: string, body?: unknown, options?: RequestInit) =>
		apiFetch(path, {
			...options,
			method: 'POST',
			headers: {
				'Content-Type': 'application/json',
				...((options?.headers as Record<string, string>) || {})
			},
			body: typeof body === 'string' ? body : JSON.stringify(body)
		}),
	put: (path: string, body?: unknown, options?: RequestInit) =>
		apiFetch(path, {
			...options,
			method: 'PUT',
			headers: {
				'Content-Type': 'application/json',
				...((options?.headers as Record<string, string>) || {})
			},
			body: typeof body === 'string' ? body : JSON.stringify(body)
		}),
	delete: (path: string, options?: RequestInit) =>
		apiFetch(path, { ...options, method: 'DELETE' })
};

export { api, getCsrfToken, storeCsrfToken, apiFetch };
