# octabot-plugins

## Zulip plugin
Allows sending messages to Zulip with a specified group and topic. Example of a task for sending a notification to Zulip:

```json
{
    "name": "Daily Notifier",
    "type": "zulip",
    "project_id": "ce15d416-fdab-4579-8b0d-e7c93ec53dbb",
    "start_at": "2025-02-11T01:00:00+03:00",
    "schedule": "0 0 15 * * 2-6 *",
    "options": {
        "channel": "Develop_test",
        "topic": "Daily meeting",
        "template": "@**all** Waiting for you at the daily meeting!"
    }
}
```

## Teamcity plugin
Allows making requests to check the status of the build configuration and publishing a message in Zulip if the build 
configuration fails. Example of a task for checking the build configuration status:

```json
{
    "name": "Teamcity build status",
    "type": "teamcity",
    "project_id": "ce15d416-fdab-4579-8b0d-e7c93ec53dbb",
    "start_at": "2025-02-11T01:00:00+03:00",
    "schedule": "0 0 15 * * 2-6 *",
    "options": {
        "channel": "Develop_test",
        "topic": "Integration test failure",
        "build_name": "Platform_Devel_Production_ReleaseDockerIntegrationTests",
        "template": "@**all** The build configuration **{name}** for the project **{project_name}** has failed with the status: *{status}*. For more details, visit [link]({web_url})"
    }
}
```

## Gerrit plugin
Allows making requests to Gerrit using Gerrit’s search capabilities, and if a non-empty list of reviews is returned, 
it publishes the list in a Zulip message. Example of a task for checking reviews:

```json
{
    "name": "Gerrit reviews",
    "type": "gerrit",
    "project_id": "ce15d416-fdab-4579-8b0d-e7c93ec53dbb",
    "start_at": "2025-02-11T01:00:00+03:00",
    "schedule": "0 0 15 * * 2-6 *",
    "options": {
      "channel": "Develop_test",
      "topic": "Stalled reviews",
      "query": "is:open age:1d -is:wip label:Verified>=0",
      "project": "platform/core",
      "template": "The following stalled reviews were found for the project {project}:\n",
      "review_template": "--- [{subject} (+{insertions}/-{deletions})]({url}/#/c/{number})\n"
    }
}
```

## Exchange plugin
The plugin checks for scheduled meetings in the calendar of a specified user, and if a meeting contains information in a specific
format, it parses it and creates a task in the bot based on the meeting data. The plugin periodically synchronizes the calendar 
state and updates tasks if a meeting is modified. Example of a task for creation:

```json
{
    "name": "Meeting",
    "type": "exchange",
    "project_id": "ce15d416-fdab-4579-8b0d-e7c93ec53dbb",
    "start_at": "2025-02-11T01:00:00+03:00",
    "schedule": "0 0/10 * * * * *",
    "options": {}
}
```
