# Private diarization fixtures

No meeting audio or RTTM labels are checked into this directory. For a local probe, keep recordings, consent/authorization evidence, model weights, diarization output, and RTTM labels outside Git. Use anonymous `sample_id` values in reports; remove names, meeting titles, absolute file paths, and other identifying metadata before sharing aggregate results.

The minimum evaluation set is 10 samples and 90 minutes, covering Mandarin, Chinese-English switching, remote-call compression, noise, one speaker, 2–6 speakers, short acknowledgments, and overlap. Keep a separate 20% holdout that is not used for tuning. A further 120-minute file is for long-duration performance, not a substitute for labeled quality samples. Confirm both recording authorization and each dataset/model's license and access conditions before use.
