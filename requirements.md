requirements.md


akmp a tool for automatically post eve online killmails to zkill


requirements:
* The tool shall be written in rust
* The tool pulls killmails from eve onlines api / esi (https://developers.eveonline.com/api-explorer#/operations/GetCharactersCharacterIdKillmailsRecent)
* The tool posts them to zkill : https://github.com/zKillboard/zKillboard/wiki/API-(Posting-Killmails)
* The tool needs to authenticate with ESI to get the correct scopes for pulling killmails.
* It should be possible to authenticate multiple characters
* The tool should not automatically post to zkillboard, the user should press a gui button to trigger that


