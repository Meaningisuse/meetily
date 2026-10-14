import type { ApiPageOptions, JobStatus, MeetingBrief, MeetingSummary, Page, SearchHit, TranscriptItem } from './contracts.js';
import { ConnectionError, type ConnectedInstance } from './connection.js';

export class AutomationClient {
  constructor(private readonly connection: ConnectedInstance, private readonly request: typeof fetch = fetch) {}

  private async get<T>(path: string, params?: URLSearchParams): Promise<T> {
    const url = new URL(path.replace(/^\//, ''), `${this.connection.baseUrl.href}/`);
    if (params) url.search = params.toString();
    let response: Response;
    try {
      response = await this.request(url, {
        method: 'GET',
        headers: { authorization: `Bearer ${this.connection.token}`, accept: 'application/json' },
        redirect: 'error',
        signal: AbortSignal.timeout(20_000),
      });
    } catch {
      throw new ConnectionError('INSTANCE_UNAVAILABLE', 'The local Meetily request could not be completed.');
    }
    if (response.status === 401 || response.status === 403) throw new ConnectionError('AUTH_REJECTED', 'The Meetily automation credential was rejected or lacks read permission.', response.status);
    if (!response.ok) {
      if (response.status === 503) throw new ConnectionError('INSTANCE_UNAVAILABLE', 'Meetily is not ready to serve local automation requests.', response.status);
      throw new ConnectionError('API_ERROR', `Meetily returned HTTP ${response.status}.`, response.status);
    }
    return await response.json() as T;
  }

  listMeetings(options: ApiPageOptions = {}): Promise<Page<MeetingBrief>> {
    return this.get('meetings', pageParams(options));
  }

  searchMeetings(query: string, options: ApiPageOptions = {}): Promise<Page<SearchHit>> {
    const params = pageParams(options);
    params.set('q', query);
    return this.get('search', params);
  }

  getTranscript(meetingId: string, options: ApiPageOptions = {}): Promise<Page<TranscriptItem>> {
    return this.get(`meetings/${encodeURIComponent(meetingId)}/transcript`, pageParams(options));
  }

  getSummary(meetingId: string): Promise<MeetingSummary> {
    return this.get(`meetings/${encodeURIComponent(meetingId)}/summary`);
  }

  getJob(jobId: string): Promise<JobStatus> {
    return this.get(`jobs/${encodeURIComponent(jobId)}`);
  }
}

function pageParams(options: ApiPageOptions): URLSearchParams {
  const params = new URLSearchParams();
  if (options.limit !== undefined) params.set('limit', String(options.limit));
  if (options.cursor !== undefined) params.set('cursor', options.cursor);
  return params;
}
