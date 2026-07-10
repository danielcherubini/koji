# System API

## GET /tama/v1/system/capabilities

Detect build prerequisites and GPU capabilities. Cached for 5 seconds.

**Response:**

```json
{
  "os": "linux",
  "arch": "x86_64",
  "git_available": true,
  "cmake_available": true,
  "compiler_available": true,
  "detected_cuda_version": "12.4",
  "supported_cuda_versions": ["11.1", "12.4", "13.1"]
}
```
