// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// In-app changelog shown in the "What's New" view. The app auto-opens it once
// after updating to a version the user hasn't seen yet (see App.tsx), and it's
// always reachable from the sidebar.
//
// ┌─────────────────────────────────────────────────────────────────────────┐
// │ RELEASE CHECKLIST: add a new entry at the TOP for every release, with the │
// │ version matching package.json / tauri.conf.json / Cargo.toml. Newest      │
// │ first. See RELEASING.md (repo root).                                      │
// └─────────────────────────────────────────────────────────────────────────┘

export interface ChangelogEntry {
  version: string; // matches the released app version, no leading "v"
  date: string; // YYYY-MM-DD
  highlights: string[]; // short user-facing bullet points
  /** True only on entries that map to an actual tagged GitHub release (RELEASING.md §2), so What's
   *  New can mark the release boundaries among the interim per-PR dev bumps. Omitted (falsy) on a
   *  dev-bump entry — which is what the top entry is on a developer machine between releases. Set
   *  when a release is cut; the `--tag` release gate refuses to tag if the top entry lacks it. */
  release?: boolean;
}

export const CHANGELOG: ChangelogEntry[] = [
  {
    version: "3.133.0-alpha",
    date: "2026-08-30",
    highlights: [
      "PM now says when it hasn\u2019t been able to read your server\u2019s context window, instead of saying nothing at all. That number is the one that explains a symptom people usually blame on the model: a server running a small window can\u2019t hold one batch of PM\u2019s background work, so PM quietly sends less per call. PM could only ever read it while a model was loaded \u2014 and nothing loads one until your first local reply \u2014 so on a new setup the warning had nothing to show and showed nothing. It now says so plainly, and it also picks the number up on its own within about half a minute of a model being loaded, by anything: your own \u2018ollama run\u2019, another app, or a session of PM you closed. That last one mattered more than it sounds, because PM forgets the number every time it starts while your server keeps the model loaded.",
      "And the number stopped being able to reassure you wrongly. Some servers report the model\u2019s own maximum rather than the window they actually loaded it with, and the two are often eight times apart. PM was showing that maximum as though it were the real setting \u2014 so a server genuinely running a small window could look comfortable and skip its own warning, while PM was quietly working to the small number anyway. Those are now told apart on screen. A figure PM had to guess is also no longer described as something your server is doing: it says PM is sizing its work that way because it couldn\u2019t read the real one.",
    ],
  },
  {
    version: "3.132.0-alpha",
    date: "2026-08-29",
    highlights: [
      "PM stopped telling Linux users they had no models while their server was busy answering with them. On Linux, Ollama installs itself as a background service that owns its models under its own account \u2014 which means PM is not allowed to read that folder. PM asked the operating system whether the folder was there, got back the same answer it gets for a folder that does not exist, and concluded Ollama was not installed. So the \u201cAlready downloaded\u201d section announced \u201cNo model folder found\u201d on a machine holding two models and serving them. It now tells the difference between a folder that is not there and one it is not allowed to open, says which it found and where, and asks your connected server what it holds instead \u2014 which is the one place that answer definitely lives. It never suggests you change a system service\u2019s permissions to make a settings page count files.",
      "The section also stopped counting the wrong thing. It only ever listed models that nothing was running, which is the right list to show \u2014 those are the ones you cannot use yet \u2014 but it also used that list as the headline count. With Ollama, everything you download is immediately available, so that number was permanently zero no matter how much you had. The count now covers everything PM can account for, a running server with nothing in it reads differently from a server that never answered, and a model whose quantization the server could not identify no longer produces a confident sentence about a quantization called \u201cunknown\u201d.",
    ],
  },
  {
    version: "3.131.0-alpha",
    date: "2026-08-29",
    highlights: [
      "Every way to run a model can now be downloaded, not just the slow one. Some models are offered two ways on the same card \u2014 the highest-quality settings, and the faster ones that fit your graphics card \u2014 and PM's Download button only ever fetched the first. On a laptop graphics card that is usually the version that runs in ordinary system memory at a few words a second, while the one you actually wanted sat on the row below with no way to get it. Both rows now have their own button, each fetching the exact file measured beside it. Where the two rows are really the same file run with different settings, PM says so instead of sending you looking for a second download that does not exist. And the copy-and-paste commands no longer vanish the moment you connect a server \u2014 they fold away, and they are still there when you want them, including the one for a different model server entirely.",
    ],
  },
  {
    version: "3.130.11-alpha",
    date: "2026-08-29",
    highlights: [
      "A second download no longer makes the first one vanish. PM refuses to run two model downloads at once, which is right \u2014 but the refusal was taking the running download's progress bar and its Cancel button down with it. The fetch itself carried on underneath a page showing no sign of it, and there was no way to watch it or stop it until you left the tab and came back. The progress now stays with the download that is actually running, and the message tells you which one that is.",
    ],
  },
  {
    version: "3.130.10-alpha",
    date: "2026-08-29",
    release: true,
    highlights: [
      "Downloading a local model actually works now, start to finish. PM has been showing you a list of models suited to your machine with no way to get any of them \u2014 the Download button existed in the code and could never appear, because not one of the seventeen carried a name your model server would recognise. They all do now, and PM fetches the exact file it measured for the card rather than a differently-packaged copy that can be a third larger. The download itself became something you can walk away from, too: it used to live inside the settings page, so switching tabs threw away the progress bar and re-armed the button, one click from starting the same multi-gigabyte fetch twice. It is now owned by the app \u2014 leave and come back and the progress is where you left it, a second copy is refused, and there is a Cancel button at last. And PM no longer calls a download a failure at the finish line: the several silent minutes your server spends verifying a large file are now given the time they need.",
      "PM stopped guessing how much your model can actually hold, and the guess had been hiding a real problem. Every model is trained for a certain amount of conversation, but the server running it decides how much to really give \u2014 and those are different numbers. Ollama in particular hands over far less than the model can take and never mentions it. PM read the model's number, so the meter that warns you a conversation is getting long, and the offer to compress it, both fire at 80% of a figure the conversation could never reach \u2014 while your older messages were being quietly dropped by the server. PM now asks the server what it is really serving, says so plainly when the answer has to be assumed, and re-checks within a minute instead of remembering the first answer forever. If PM told you to raise your context length and you did exactly that, it now notices.",
      "The same honesty went into the work PM does in the background. It was sending its sorting proposals and summaries in single lumps far larger than a local server can hold \u2014 and a server does not refuse those, it silently throws away the beginning and answers anyway. The beginning is where PM's instructions are. Work is now sized to the room your server really has before it is sent, PM only marks off the part it actually managed, and where something cannot be made to fit it stops and shows you the two numbers rather than pretending.",
      "A reply that failed is no longer written down as an answer. A cut-off or blank reply from a struggling local model was being recorded as a real result in several places, and in three of them that was permanent: a truncated summary was folded into your conversation's running summary and the messages behind it never read again, a cut-off pass over your history marked those messages as scanned so anything you had said about how you like things done was lost with them, and the one-time import of your old profile notes could stamp itself complete having imported nothing. None of those count as an answer now \u2014 PM leaves the work where it is, tries again next time, and tells you which of the two happened. Billed calls whose replies were rejected also always reach the usage log, which they did not before.",
      "Under the hood, two security checks that had never actually been run. PM boxes its document processor in so it can only reach the handful of folders it needs \u2014 but that confinement only exists on Linux and PM was built on Windows, so nothing had ever tested that it confines anything, and it fails silently if it stops working. It has now been run for real on a Linux machine, along with the on-device engine that reads text out of your photos, which runs inside the same box; a photo of a receipt failing there would simply have been filed as holding no text, looking entirely normal. Both work, and both now have tests that will say so from now on.",
    ],
  },
  {
    version: "3.130.9-alpha",
    date: "2026-08-28",
    highlights: [
      "More under-the-hood tidying on the same Linux thread as the last update. PM reads text out of your photos with an on-device engine, and that engine runs inside the same locked-down helper as everything else that opens a file you did not write. Whether it could still work in there had never actually been checked on a real Linux machine \u2014 and the way it fails is silent: a photo of a receipt would simply be filed as holding no text at all, looking completely normal. It has now been run for real, it works, and there is a test that will say so from now on.",
      "Also fixed: the instructions for running those Linux and Mac checks were wrong in two ways \u2014 a mistyped flag and a folder name PM has never used \u2014 so anyone following them got an error instead of a result.",
    ],
  },
  {
    version: "3.130.8-alpha",
    date: "2026-08-28",
    highlights: [
      "Under-the-hood tidying, with one thing worth naming: the Linux file confinement around PM's document processor is now actually tested. PM boxes that helper in so it can only read the handful of folders it needs and nothing else — but that boxing-in had never been exercised by a test, because it only exists on Linux and PM was built on Windows. There is now a test that runs the helper for real and checks it can read what it should and genuinely cannot read anything else.",
      "Also: the contributor guide now covers licence headers and how a file records more than one author, ahead of a second developer joining.",
    ],
  },
  {
    version: "3.130.7-alpha",
    date: "2026-08-28",
    highlights: [
      "A model download now survives you leaving the tab. It used to live inside the settings page itself, so switching tabs mid-download threw away the progress display and re-armed the Download button — one click away from starting the same multi-gigabyte fetch twice. The download is now owned by the app: come back and the progress bar is where you left it, a second copy of the same download is refused, and there is a Cancel button at last.",
      'Big downloads stopped being called failures at the finish line. After fetching, Ollama checks the file in silence — several minutes for the largest models — and PM\'s patience ran out at two, so it reported "the download stalled" about a download that was essentially done. The checking phase now gets the time it needs; a genuinely silent download is still called out.',
      'Honesty and polish around the edges: the two-configurations card no longer claims Download fetches "the runner\'s default" file (it fetches exactly the Highest-quality file measured — that claim predated the fix and said the opposite); the sidebar updates the moment you assign a model to a role instead of after the next call; the served-models list refreshes on its own, so "download a model and it appears here" is now true while you watch; the cooldown chip counts down instead of freezing; the progress bar no longer reads full while the download is only fetching its manifest; and the Already-downloaded section explains itself in help mode.',
      "Guide corrections: LM Studio's Linux entry now mentions the processor requirement its Windows entry already had, and the Ollama guide now says to press Auto-detect when you come back rather than promising PM notices by itself (it only does during first-run onboarding). The release notes for the two versions above were also trimmed where they promised slightly more than shipped.",
    ],
  },
  {
    version: "3.130.6-alpha",
    date: "2026-08-28",
    highlights: [
      'The last release taught PM to stop writing a broken reply down as an answer. The audit found the places that lesson missed. The most important: the one-time import of your old profile notes checked that the reply looked like a list, but not that the list could actually be read — so a reply with one stray comma in it still counted as "your notes contained nothing", and the import stamped itself finished over zero results, permanently. Reading the list properly is now part of the check, there and in the pass that learns preferences from your chats.',
      "Three more readers got the same discipline: project auto-triage no longer accepts an unfinished reply as a real proposal (or blames its own output parsing for one), the retrieval diagnosis no longer presents a blank reply as an empty diagnosis, and the add-a-preference form no longer answers a cut-off model with “try rephrasing” — no rephrasing fixes a reply that never finished.",
      "A local server that answers every call with a blank reply — a broken model setup, typically — used to have its failure record wiped by each of those “successful” answers, so PM never escalated. Blank now counts the same as cut-off: the server is noted as answering but not delivering.",
      "And billed calls whose replies were rejected are now always in the usage log — three places dropped the spend record along with the bad reply.",
    ],
  },
  {
    version: "3.130.5-alpha",
    date: "2026-08-28",
    highlights: [
      "A pre-release audit of the local AI work closed the gaps the last round left. The biggest: the size-to-fit protection quietly stood down in exactly the cases that needed it most. A server with a very small memory was handed the full-size work the protection exists to shrink, and on two of the three server kinds PM could mistake a model's advertised capacity for the memory actually in use and size its work up to eight times too large. Both now size honestly: small stays guarded, and an advertised capacity is never trusted over a measured one.",
      "PM also now re-checks a server's memory instead of remembering the first answer forever. Before, if PM told you to raise the context length and you did — restarting the server exactly as instructed — PM kept refusing based on the dead server's number until you restarted PM too. Now it notices within a minute, in both directions: a server that came back bigger stops being refused, and one that came back smaller stops being overfed.",
      "A single enormous chat message — a pasted document, say — could silently stop that conversation's summary and preference-learning forever, with everything after it stuck behind it. PM now reads as much of the oversized message as the server can hold and moves on, rather than wedging.",
      'Smaller honesty fixes in the same area: the sidebar now shows a known context window even when one of the two roles hasn\'t answered yet; when a chat message is refused for size, the message mentions Compress — the fix you can do from inside PM — alongside the server setting; and the token figure in that message now says "up to about", because it is a deliberate overestimate.',
    ],
  },
  {
    version: "3.130.4-alpha",
    date: "2026-08-27",
    highlights: [
      'PM could not tell a bad answer from an empty one, and it wrote both down as an answer. When a model gets cut off mid-sentence, what comes back looks the same as a model that simply found nothing to say — the same successful response, with text attached. PM\'s readers are built to shrug and carry on rather than complain, so a half-finished reply quietly became "there was nothing here", and PM then marked that work done and moved past it.',
      "That mattered most where moving past it is permanent. A cut-off summary was folded into your conversation's running summary and the messages behind it were never read again. A cut-off pass over your chat history marked those messages as scanned, so anything you had said about how you like things done was lost with them. The one-time import of your old profile notes stamped itself as complete and would never have run again. A cut-off daily briefing was saved as the answer for today's facts, so nothing would have replaced it until those facts changed.",
      "None of those now count as an answer. PM leaves the work where it is and tries again next time, and says which of the two happened rather than reporting nothing found.",
      'One more, and this one was rewriting your files: in the whole-library re-tagging pass, a model that ignored the tag list and invented its own labels had all of them thrown away — correctly — and the document then came back with no tags at all, which PM offered as "remove every tag from this document". A model saying "none of these fit" is a real answer and still does that. A model that never used the list is not, and now proposes nothing.',
    ],
  },
  {
    version: "3.130.3-alpha",
    date: "2026-08-27",
    highlights: [
      "PM was sending its background work — sorting proposals, summaries, the things it learns about how you file — in single lumps far larger than a local server can hold. What happens then is the part worth knowing: the server does not refuse. It quietly throws away the beginning of the message and answers anyway. The beginning is where PM's instructions are, so what arrives is a pile of your documents with nothing telling the model what to do with them, and nothing telling it that the documents are not instructions.",
      "PM now sizes its work to the room your server actually has — measured when it can, assumed conservatively until then — and works within it. It sends fewer documents per batch on a small server and the full batch on a large one, and it decides that before it sends rather than finding out afterwards. Where it cannot make something fit, it stops and tells you the two numbers — what the job needed, and what your server is serving — along with the one setting that changes it. It never fails your server for this; the size of the message was PM's choice, not your machine's fault.",
      "It also stops advancing past work it did not really do. When a batch has to shrink, PM only marks off the part it actually sent, so the rest is picked up next time instead of being skipped forever.",
      "And the Local AI tab now tells you the number outright when it is small. Ollama gives 4,096 tokens unless you tell it otherwise, and never mentions it — so the setup instructions for all three servers now include the line that raises it.",
      "One thing found while measuring, and fixed here: a document title can contain a line break, and PM was putting titles straight into a list where each line means something. A file named to look like PM's own text could add a line to that list. Titles and folder names are now kept to one line, and shortened — which is also what stops four hundred long filenames from filling a small server on their own.",
    ],
  },
  {
    version: "3.130.2-alpha",
    date: "2026-08-27",
    highlights: [
      "PM was measuring your local model's context against a number it made up. Every model is trained to handle a certain amount of conversation at once, but the server running it decides how much to actually give it — and those are different numbers. Ollama in particular usually gives far less than the model can take, and never mentions it. PM read the model's number, so on a typical laptop it believed it had eight times the room it really had.",
      "That is worse than a wrong percentage. The warning that tells you a conversation is getting too long, and the offer to compress it, both fire at 80% — and with the wrong number underneath, the meter could never reach 80% no matter how long the conversation got. So the warning could not fire at all, while your older messages were being quietly dropped by the server. PM now asks Ollama what it actually loaded, and asks llama-server the same question it always did.",
      "And when it genuinely cannot find out, it says so instead of guessing. It falls back to a small, safe figure and the context panel tells you the number is assumed. A model's own specification is no longer treated as an answer to a question only your server can answer.",
    ],
  },
  {
    version: "3.130.1-alpha",
    date: "2026-08-27",
    highlights: [
      'The model list at the bottom of the sidebar was naming the wrong model. If you had pointed PM at a model on your own machine, it kept showing the cloud model\'s name — and put "Local: connected" directly underneath, which read as "cloud is doing the work, your machine is standing by" when the exact opposite was true. Each row now names whatever will actually answer, and hovering tells you what\'s behind it as a fallback.',
      "And a local server that answers with something PM doesn't understand no longer claims a stream broke. That message appeared for checks that never streamed anything, and it threw away the one useful part — what the server actually said. PM now says plainly that it couldn't read the answer, shows you the answer, and stops treating a server that replied as one that died.",
    ],
  },
  {
    version: "3.130.0-alpha",
    date: "2026-08-27",
    highlights: [
      "Every recommended model now has a working way in. PM has been showing you a list of models it thought would suit your machine, with no way to get any of them — the Download button was there in the code and could never appear, because the list carried no name Ollama would recognise for a single one of them. All seventeen models now carry one. Pick one, press Download, and your own Ollama fetches it — the button appears once an Ollama endpoint is connected, and the few very largest size variants that come split into pieces (which Ollama refuses) say so on their card and point you elsewhere instead.",
      "It fetches exactly the file PM measured, too. The sizes and speeds on each card were worked out from a specific file, and the same model packaged elsewhere can be a third larger — enough to turn a model that fits into one that doesn't. PM now downloads from the same place it took its measurements, so what the card promised is what lands on your disk. Where it can't do that honestly it offers no button and says why instead.",
      'And the list stops contradicting itself after you use it. Downloading a model used to leave the "already downloaded" section showing the picture from before you pressed the button, until you restarted PM. It also told anyone with a model folder but nothing in it that everything they had was already loaded — which is what you see the moment you delete your last model, or the first time you install one of these apps.',
    ],
  },
  {
    version: "3.129.3-alpha",
    date: "2026-08-26",
    highlights: [
      'PM couldn\'t find Ollama on your own machine until you had already downloaded a model into it. Asked which models it has, a freshly installed Ollama answers "none" in a particular way, and PM read that answer as "this isn\'t a model server at all" — so auto-detect kept telling you nothing was installed while your server was running and answering it every time. Which is exactly the wrong way round: a server you have only just installed has nothing in it yet, so this landed on people at the one moment it mattered most. PM now recognises a working server with nothing in it, and says so.',
      "It also tells you what to do about that. A connected server with no models used to report as a clean pass, in green, and then leave you with two empty dropdowns and nothing anywhere explaining why. It now reads as something still to do, and says plainly that a model needs downloading into it before PM has anywhere to send work.",
    ],
  },
  {
    version: "3.129.2-alpha",
    date: "2026-08-26",
    highlights: [
      "Under-the-hood tidying: the library that gives everything PM stores its own internal identifier moved up a version. It is the same library that moved earlier today — a newer release arrived a few hours behind the last one. Nothing it adds is anything PM asks of it, and nothing about the app you use changes.",
    ],
  },
  {
    version: "3.129.1-alpha",
    date: "2026-08-26",
    release: true,
    highlights: [
      "Running an AI model on your own machine is now something PM can walk you through. It works with three local servers — Ollama, LM Studio and llama-server — but until now it had instructions for only one of them, and a recommended model you hadn't downloaded yet gave you no way to get it. All three now have proper instructions for your operating system, alongside what each one is good at, how models get into it, and the thing most likely to rule it out for you. Every recommended model shows a command that fetches and runs it. Every one of those commands was checked against the makers' own documentation rather than written from memory. If you have ever wanted PM to keep working without an internet connection, this is the release to try it on.",
      "The numbers PM showed you about your own machine are now true. It worked out how much memory you had free once, when you opened the tab, then judged every model against that one reading for the rest of the session — so opening PM while your machine was busy could rule out a model that fits perfectly well. On Linux it never recognised your graphics card at all. And laptop graphics were borrowing the speed of the desktop card that shares their name, which overstated some laptops by around double. All three are fixed, seventeen laptop chips now carry their own verified figures, and a card PM has no real figure for says so rather than guessing. A model that only just fits now tells you that it only just fits.",
      "Two things that made a perfectly good local model look broken. If your machine has a proxy set up — common behind a work network, or with a VPN client running — PM was sending requests to your own computer through it, which cannot work; after three failures it concluded your model was dead and stopped using it for minutes at a time. And the Local AI status light was showing green from a guess rather than from anything it had actually seen, so it could sit there reassuring you while chat was failing. Requests to your own machine now go straight there, and the light reports what really happened.",
      "Elsewhere: a document containing a single em dash or accented name no longer becomes unreadable, and no longer holds up everything queued behind it. Transcribing a recording is back to full speed on a laptop with four processor cores or fewer. Help mode stopped explaining a project status PM removed a long time ago, and now checks itself against the statuses PM really has, so a retired one cannot quietly survive there again. The libraries behind reading documents, on-device search and voice notes all moved up to current versions, with every licence read again before it went in.",
    ],
  },
  {
    version: "3.129.0-alpha",
    date: "2026-08-26",
    highlights: [
      "Local AI now explains the choice it was quietly asking you to make. PM works with three local servers — Ollama, LM Studio and llama-server — but only ever gave you instructions for one of them and a single line conceding the others existed, which is no help at all if you have never installed any. All three now get proper instructions for your operating system, alongside what each is actually good for, how you get models into it, and the thing most likely to rule it out for you. Every command was checked against the makers' own documentation rather than written from memory.",
      "That comparison also stays reachable after you have connected something. It used to disappear the moment you had a server set up, which is roughly when you start wondering whether one of the others would have suited you better.",
      "Every recommended model now tells you how to get it. Before this, a model you hadn't downloaded offered nothing at all — no download button and no command — because PM's list of models carries no Ollama names to hand its downloader. Each one now shows a single command that fetches and runs it, and points you at the right place to find it in the other two apps. The three name models differently enough that a command from one does nothing in another, so PM only shows a command where it knows it is right.",
    ],
  },
  {
    version: "3.128.15-alpha",
    date: "2026-08-26",
    highlights: [
      "If your machine has a proxy set up — common behind a work network or with a VPN client running — PM was sending its requests to your own local AI server through it. A proxy has no route back to your own machine, so the request failed, and after three of those PM concluded your local model was dead and stopped using it for a few minutes at a time. Requests to a server on your own machine now go there directly, as they always should have. A local server you have deliberately pointed at another machine still uses your proxy, because that one is a real network destination.",
      "The Local AI status light no longer shows green while chat is failing. PM only checks your server every thirty seconds, and in between it was guessing from whether the model was in a cooldown — so a server that had just failed, but not yet failed often enough to be put in one, still showed as connected. It now reports what was actually last seen, and a server that has never once answered no longer shows green at all. Real chat traffic counts as a check, so the light is also more current than it was.",
    ],
  },
  {
    version: "3.128.14-alpha",
    date: "2026-08-26",
    highlights: [
      "Local AI Workbench: several of the numbers PM showed about your machine weren't true, and they are now. The biggest is that PM worked out how much memory you had free once, when you first opened the tab, and then judged every model against that figure for the rest of the session — so opening PM while your machine was busy could rule out a model that fits perfectly well an hour later. That reading is now taken fresh each time PM scores models; the slower checks, like your graphics card, stay cached as before.",
      "On Linux, PM never recognised your graphics card. It knew one was there and how much memory it had, but not which model — so the speed estimates always fell back to a generic figure, and the readout said so. PM now reads the card's name and works the speed out from that card's real specification.",
      "Laptop graphics also stopped borrowing a desktop card's numbers. A laptop chip shares its name with a desktop card and almost never its memory — a laptop RTX 4070 has less than half the desktop one's — so PM was overstating some laptops' speed by around double. Seventeen laptop chips now carry their own verified figures, and any laptop card PM doesn't have a real figure for keeps the generic estimate and says so, rather than borrowing a number that isn't its.",
      "A model that had to shrink its context to fit now also tells you when it barely fits. Those two things were competing for one line, so the models scraping the very bottom of your memory were the ones saying nothing about it.",
      'The "vision" label is gone from the model list. It was true of the models, but not of PM: chat messages carry text only, so PM cannot send a picture to any model, local or cloud. PM reads images a different way entirely. Labelling it made a heavier model look more useful for PM\'s purposes than a lighter one, which is backwards.',
      "PM also no longer refuses to size a model just because the publisher didn't say how big its unused image component was — everything else about it was perfectly measurable.",
      "If you give Chat and Background two different local models, PM now says plainly that both live on the same server at the same time, so your machine holds both. Every fit PM shows is for one model on its own, and two that each fit alone may not fit together.",
      "Machines without a separate graphics card — including Apple Silicon — are now told how much memory PM keeps free when sizing models. That line only ever appeared next to the graphics-card figure, so the people with no graphics-card figure never saw it.",
    ],
  },
  {
    version: "3.128.13-alpha",
    date: "2026-08-26",
    highlights: [
      "Help mode was still teaching a project status PM no longer has. \"Part of\" was removed a long time ago — it hid a project's own status behind a parent's name, and Merge into… does honestly what it was really being used for — but four help notes and a button tooltip went on describing it. One still told you to set a parent that is no longer a field, and two promised the AI would suggest one, which it is no longer even asked to do. All five now describe what PM actually offers. PM will also notice next time: the help text for statuses is now checked against the statuses PM can really show, so a retired one cannot quietly survive in the help again.",
    ],
  },
  {
    version: "3.128.12-alpha",
    date: "2026-08-26",
    highlights: [
      "Two of the models PM suggests for running on your own machine were listed as bigger than they really are. Some publishers ship an optional extra file alongside a model to help it generate faster, and PM was counting that file as part of the model itself — so both Gemma 4 models looked around half a gigabyte heavier at their highest quality, and were described as arriving in several pieces when they arrive in one. On a machine where those models sat near the edge of what fits, that was the difference between PM recommending one and warning you off it. Both are now listed at their real size, and PM will offer to re-check your machine against the corrected figures.",
      "The rest of the list was checked against Hugging Face the same day and was already current — no other model's size, quality options or context length had moved.",
    ],
  },
  {
    version: "3.128.11-alpha",
    date: "2026-08-26",
    highlights: [
      "The part of PM that reads your documents — the one that opens PDFs, Office files and images and turns them into something PM can search — has had its whole set of underlying libraries moved up to current versions. That includes the document reader itself and the engine behind on-device search and voice notes. This is maintenance rather than a new capability: nothing you do in PM changes, and the licence behind every one of those libraries was read again before it went in.",
    ],
  },
  {
    version: "3.128.10-alpha",
    date: "2026-08-26",
    highlights: [
      "Under-the-hood tidying: two of the libraries PM is built from moved up to their latest patch releases — the one that gives everything PM stores its own internal identifier, and one that only ever runs when PM's own tests do. Nothing about the app you use changes.",
    ],
  },
  {
    version: "3.128.9-alpha",
    date: "2026-08-20",
    highlights: [
      "Transcribing a recording is back to full speed on a smaller machine. The change that stopped PM hogging your processor set a limit that a second part of PM — the one that turns speech into text — quietly read as its own, and took roughly half the processors it had been using on a laptop with four cores or fewer. Machines with eight or more were unaffected, and larger ones were already faster. Transcription now works out its own figure: never fewer than it used before, and more on a machine that has it to give.",
      "A correction to what the last release said. PM stepping back when you are doing something else covers everything that part of PM does — reading and filing new documents, yes, but also the work behind a chat answer or a search. The note described it as indexing alone, which undersold where it applies.",
    ],
  },
  {
    version: "3.128.8-alpha",
    date: "2026-08-20",
    highlights: [
      "A dash or an accented name no longer stops PM reading a document. PM works out what alphabet a file is written in by looking at its first few kilobytes, and a file that happens to open with plain English was being treated as though all of it were plain English — so the first em dash, curly quote or accented name further down made the whole document unreadable. It was worse than losing one file: PM kept handing the same one back to itself instead of moving on, so a single document could quietly hold up everything queued behind it. PM now re-reads those files properly, and a file genuinely written in an alphabet it cannot read is set aside once rather than tried forever.",
    ],
  },
  {
    version: "3.128.7-alpha",
    date: "2026-08-20",
    release: true,
    highlights: [
      "PM stops holding on to memory it has finished with. The part of PM that reads your documents and turns them into something searchable was asking for far more memory at a time than the work needed, and then never giving it back — so a session left open all day could end up sitting on five gigabytes it had no further use for. On a laptop that is the difference between PM being one open app among many and PM being the reason everything else starts swapping. It now works in batches sized to your machine and hands back what it borrows as each batch finishes. It turned out to be quicker this way too: the oversized batches were slower, not faster.",
      "PM gets out of the way when you are doing something else. Indexing used to reach for every processor core on the machine at once, which is why opening PM could make everything else feel sluggish while it caught up on your files. That work now runs at a lower priority than whatever you are actually working in: it still uses the whole machine when nothing else wants it, and steps back the moment you do. There is nothing to switch on, and it applies whether you have indexing set to Fast or to Gentle.",
      "Under the hood, fifteen of the libraries PM is built from moved up to their latest patch releases — among them the database layer your library is stored in, and the hashing and encoding PM uses to keep track of your files. Building PM on a developer's machine also stopped leaving behind tens of gigabytes of debugging detail nobody needed. Nothing about the app you run changes.",
    ],
  },
  {
    version: "3.128.6-alpha",
    date: "2026-08-20",
    highlights: [
      "PM stops holding on to memory it has finished with. The part of PM that reads your documents and turns them into something searchable was asking for far more memory at a time than the work needed — and then never giving it back, so a session left open all day could end up sitting on five gigabytes it had no further use for. It now works in batches sized to your machine and hands back what it borrows when each batch is done. On a laptop that is the difference between a couple of hundred megabytes and several gigabytes, and it is the difference between PM being one open app among many and PM being the reason everything else starts swapping. It turned out to be quicker this way too — the oversized batches were slower, not faster.",
      "PM gets out of the way when you are doing something else. Indexing used to reach for every processor core on the machine at once, which is why opening PM could make everything else feel sluggish while it caught up on your files. That work now runs at a lower priority than whatever you are actually working in: it still uses the whole machine when nothing else wants it, and steps back the moment you do. There is nothing to switch on, and it applies whether you have indexing set to Fast or to Gentle.",
    ],
  },
  {
    version: "3.128.5-alpha",
    date: "2026-08-20",
    highlights: [
      "Under-the-hood tidying: fifteen of the libraries and build tools PM is made from moved up to their latest patch releases — among them the database layer your library is stored in, and the hashing and encoding PM uses to keep track of your files. The rest are the tools that build and check the app, which never reach your machine at all. Nothing about the app you run changes.",
    ],
  },
  {
    version: "3.128.4-alpha",
    date: "2026-08-07",
    highlights: [
      "Under-the-hood tidying, with nothing to see. Building PM on a developer's machine was leaving behind far more than it needed to — one checkout had quietly grown to 123 GB of build leftovers, most of it debugging detail kept from every version of the code ever compiled there. Development builds now keep only the part that makes a crash report readable and drop the rest. Nothing about the app you run changes.",
    ],
  },
  {
    version: "3.128.3-alpha",
    date: "2026-08-06",
    release: true,
    highlights: [
      "A folder on the pinboard can pick your next task for you. Put the jobs you're dithering between into one folder, press the dice in its top bar, and choose how it should decide: a roulette wheel, a fist of straws, a box of folded slips, a coin toss, or rock-paper-scissors against PM. The first three pick one card out of all of them and the last two put a single card to you and let you play for it — lose and it's yours, win and you're off the hook. Whatever it lands on is what you do next. Nothing checks up on you afterwards; it is there to be argued with, not obeyed.",
      "Each game is properly played rather than announced. The wheel turns several times and slows into its wedge, the straws are pulled from a fist to reveal the long one, the box is shaken with the slips jostling inside before one is lifted out, the coin is thrown in an arc and turns over as it goes, and a throw is counted out with both fists before either opens. Every piece carries the name of the card it stands for — along its wedge, down its straw, on the slip that comes out — and a folder opened as an overlay draws all of it larger, with room for longer names. If you have motion turned down in Settings, the games skip straight to the answer.",
      "A card the folder has picked greys out and waits its turn, and isn't offered again until every other card has had one — then the round loops back and starts over. That round is remembered between visits, so you can shut PM, go out for the afternoon, and come back to a folder that still knows what it has already handed you. If you would rather have no memory between plays at all — every card in every draw, the same one twice running — there's a switch for that, next to the one that sends a chosen card straight to your board. On the wheel, cards can also be given a bigger or smaller share, and the wedges are cut from exactly those shares, so it can never show you one thing and pick by another.",
      "Every document now says where it actually is: the folders it sits in, the way Google Drive shows them — “My Drive › Projects › PM › documentation”, or “Shared with you › crisis › study guide”. That replaces a web address which could tell you how to open a file and never where it lived, so two files called “Notes” looked identical. A file PM has found in more than one place shows a separate trail for each, which is the difference that matters on the screen asking you whether they're duplicates. Where PM cannot see the whole trail it shows what it can rather than guessing — the folders above something shared with you belong to whoever shared it — and the address is still one click away, with a Copy button.",
      "A Documents table that stops wasting space and keeps the shape you give it. Each column is the width of what's in it rather than reserving room for its worst case, and all that slack used to collect in one gap between a title and the buttons beside it. Sorting by size and going to look at something else no longer drops you back to newest-first when you come back, and it survives closing PM. There's a new Source column too, off until you switch it on, saying what PM is holding and what it's only pointing at — and clicking its heading gathers a whole library's worth of trouble at one end.",
      "A note can tell a bullet from a dash. Starting a line with . gives a round bullet and - now gives an en dash — two kinds of point instead of one, so a list and the asides hanging off it don't have to look the same. Both stay proper list items, so they nest, and one long enough to wrap continues under its own text. A checklist sits flush against the note's edge, the bullets sharing a list with it keep their dots, and code pasted into a note is left exactly as pasted — a line beginning with a dash inside a code block stays a line beginning with a dash, which in a diff is the difference between a line being removed and a line being added.",
      "A new line in a note is brought into view before you type into it. When PM continues a list for you it places the cursor itself, and a browser only follows a cursor it moved on its own — so on a note card a few lines tall you were typing into a line just below the bottom edge. Tab, the formatting buttons and undo all place the cursor the same way and are all fixed with it, in both directions: the note no longer scrolls itself when the line was already showing.",
      "Smaller things you'll see. The badges in the Documents list line up down the right of the title column again, so a file with something wrong with it is findable by scanning one column instead of reading every row. The Edit button on a row stays out of the way until you hover it, like Delete beside it. And sorting by a column most documents have no answer for keeps the blanks at the bottom whichever way you sort.",
      "Under the hood, a review of everything in this release closed fourteen faults before any of it reached you — among them a folder game that moved a card onto your board after telling you you'd won it, a tick-list that had quietly lost its alignment, notes that scrolled themselves whenever you pressed Enter near the top, and a document's folder trail that could be saved wrong and stay wrong if Google was busy at the moment PM asked.",
    ],
  },
  {
    version: "3.128.2-alpha",
    date: "2026-08-06",
    highlights: [
      "A card you WIN at a coin toss or at rock, paper, scissors stays in its folder. It was being moved onto your board anyway, if you had asked for winners to be moved out — so the screen said you were off the hook while the job quietly landed on the board regardless. Winning means you don't have to do it, which is the entire point of playing for it.",
      "A tick-list in a note sits flush against the note's edge again, rather than indented as though it were nested inside something.",
      "Notes stop scrolling themselves. Pressing Enter, Tab or a formatting button near the top of a note pushed the note down a line and took its first line off the top of the card; and an undo that put the cursor back near the top left it just above the edge instead of scrolling up to it. Both are the same miscount, in opposite directions.",
      "Code pasted into a note is left exactly as pasted. A line beginning with a dash inside a fenced code block was being rewritten as a dash point, which in a diff turns a removed line into an added one — the snippet said the opposite of what was pasted, in the note and in the copy filed into your library. Tick-boxes drawn inside code are no longer offered as tick-boxes either, so the real ones stay in step.",
      "A document's folder trail is either right or absent, never plausibly wrong. If PM was throttled or briefly couldn't reach a folder part-way up the chain, it saved the short path it had got as though that were the whole answer — and a saved trail is never looked at again, so `Clients › Acme › Invoices` stayed `Acme › Invoices` for good. It now waits and asks again on the next sync. With two Google accounts connected, one account being refused a folder no longer answers the question on the other's behalf.",
      "Smaller things around the folder games: switching game mid-spin no longer announces a result for a game you've left, taking a card out of a folder by hand or deleting it now takes it out of the round too (the count could read '-1 of 1 still in'), and choosing a game or flipping either of its switches no longer costs you a Ctrl+Z that does nothing.",
    ],
  },
  {
    version: "3.128.1-alpha",
    date: "2026-08-06",
    highlights: [
      "The folder games now actually play. The wheel spins — several turns, slowing into the wedge it lands on — the straws are pulled from a fist to reveal the long one, and the box is given a proper shake, slips jostling inside it, before one is lifted out. Every piece carries the name of the card it stands for: written along its wedge, down its straw, on the slip that comes out. The coin is thrown in an arc and turns over as it goes, and a throw of rock, paper, scissors is counted out with both fists before either opens.",
      "Smaller things around the games. With one card left there is nothing to gamble on, so the folder simply offers it rather than spinning a wheel with one wedge; and once you have taken the last one, the button that was 'Go again' says what it now does, which is start the round over. 'Move the winner out' has moved down beside 'Start the round over', where the rest of the round lives, and the list of games has gained 'Just a folder' at the top — one obvious way back to an ordinary folder, instead of having to press the game you are playing a second time.",
      "The games are now drawn to fit where they are. A folder opened in place gets a fixed panel; opened as an overlay it gets most of the board, so the wheel, the straws, the box, the coin and the hands are all drawn larger there, with room for longer names at a readable size instead of the same three words set bigger. And the list of what is still in no longer gives the game away: a card the wheel has landed on stays un-greyed, unticked and counted until the wheel actually stops.",
      "And you can now turn the taking-turns off. 'Grey out what it picks', under the game, is what makes a round a round: leave it on and a card waits until every other one has had a go, or switch it off and there is no memory between plays at all — every card is in every draw and the same one can come up twice running. A fair share-out or an honest coin toss, whichever you actually wanted.",
      "Three fixes. A card the folder had already drawn showed a stray piece of markup instead of its tick. A folder's own board came back without its ruled grid after a visit to the game. And a coin folder and a rock-paper-scissors folder both wore the box's icon on the board rather than their own.",
    ],
  },
  {
    version: "3.128.0-alpha",
    date: "2026-08-06",
    highlights: [
      "Two more ways for a folder to settle it, and these two play you rather than picking for you. Flip a coin and heads the job is yours, tails you’re off the hook. Or throw rock, paper, scissors against PM: it puts a card on the table, you throw, and you only have to do it if PM wins. Nothing checks up on you afterwards — it is there to be argued with, not obeyed. Either way that card has had its turn, so the folder moves on to the next one instead of putting the same job to you all afternoon, and a card you dodged is never moved out to your board, because dodging it is the opposite of being given it.",
      "On the roulette wheel, some cards can matter more than others. Each one starts on an even share and can be nudged up to three times that or down to a quarter, from the list under the wheel. The wedges are cut from exactly those shares, so a wheel can never show you one thing and pick by another — and a folder nobody has tuned spins exactly as it did before. The other games don’t offer it: a straw’s length is already the answer, and a coin has two faces and one card, so there is no share to hand out.",
    ],
  },
  {
    version: "3.127.0-alpha",
    date: "2026-08-06",
    highlights: [
      "A folder on the pinboard can pick your next task for you. Put the jobs you're dithering between into a folder, press the dice in its top bar, and choose a game: a roulette wheel, a fist of straws, or a box of folded slips. Each has a piece for every note in the folder, each picks one entirely at random, and whatever it lands on is what you do next. The folder's tile on the board changes to show which game it plays and how many cards are still in, and clicking it plays rather than opening — 'Cards' in the top bar is always the way back to the notes themselves.",
      "A card the folder has already picked greys out and stays where it is, and isn't offered again until every other card has had a turn — then the round loops all the way back and starts over. That round is remembered between visits, so you can shut PM, go out for the afternoon, and come back to a folder that still knows what it has already handed you rather than one that starts again from the top. If you would rather a chosen card left the folder for the board, there's a switch for that beside the games.",
      "Undoing something else on the board no longer un-picks what a game already drew. Timelines can live in a game folder but are never drawn — a dated track isn't a task somebody can be told to go and do. And if you have motion turned down in Settings, the games skip the spin and go straight to the answer.",
    ],
  },
  {
    version: "3.126.1-alpha",
    date: "2026-08-05",
    highlights: [
      "The badges in the Documents list line up down the right of the title column again, so a file with something wrong with it is findable by scanning one column instead of reading every row. They had drifted in beside each title when the column stopped stretching, which fixed the gap and lost the alignment; the gap stays fixed and the alignment comes back.",
      "The Edit button on a document row stays out of the way until you hover the row, like Delete beside it — and stays visible while its own panel is open, so there is always something to click to close it. It is still reachable by keyboard without hovering anything.",
    ],
  },
  {
    version: "3.126.0-alpha",
    date: "2026-08-05",
    highlights: [
      "Every document says which folders it sits in, the way Google Drive does — “My Drive › Projects › PM › documentation”, or “Shared with you › crisis › study guide”. It replaces the web address that used to sit there, which could tell you how to open a file and never where it was, so two files called “Notes” looked identical. The reader shows the trail too, and the address is still one click away: the caret beside “Open source” shows it in full, with a Copy button for when you want to send someone the link.",
      "A file PM has found in more than one place shows a separate trail for each. That is the difference that matters when you are deciding whether something really is a duplicate — the same Drive file is “My Drive › Projects › Q3” to you and “Shared with you › crisis” to someone it was shared with, and until now those two copies read exactly alike on the one screen that asks you to delete one of them.",
      "Where PM cannot see the whole trail, it shows what it can rather than guessing. Folders above something shared with you belong to whoever shared it, so the trail starts where your view of it does. Files already in your library fill their folders in as PM next syncs them; OneDrive shows the one folder it reports, and Drive files show the full path.",
    ],
  },
  {
    version: "3.125.0-alpha",
    date: "2026-08-05",
    highlights: [
      "A note can tell a bullet from a dash. Starting a line with . gives you a round bullet and starting it with - now gives you an en dash — two kinds of point instead of one, so a list and the asides hanging off it don't have to look the same. Dash points are still proper list items: they nest under a bullet, and one long enough to wrap continues under its own text rather than back at the margin. Notes you have already written change shape once, wherever you used a dash.",
      "A bullet sharing a list with a checkbox keeps its bullet. A note's checklist deliberately sits flush with the note's left edge, and a round bullet's dot is drawn in exactly the space that removes — so any ordinary bullet in the same run as a checkbox quietly lost its dot and read as a stray line of prose. The checklist still sits flush; the bullets beside it get their dot back. Lines you typed one after another also stay one block, instead of opening a gap wherever you switch between bullets, dashes and checkboxes.",
      "A new line in a note is brought into view before you type into it. When PM continues a list for you — the next bullet, the next number, a fresh checkbox — it places the cursor itself, and a browser only follows a cursor it moved on its own. On a note card a few lines tall that meant typing into a line just below the bottom edge. Tab, the formatting buttons and undo all placed the cursor the same way and are all fixed with it.",
    ],
  },
  {
    version: "3.124.0-alpha",
    date: "2026-08-05",
    highlights: [
      "Every document now says where it actually is, in the table AND when you open it. A file on your machine always showed its path under its title; anything indexed from a connected account showed nothing at all — including files in a folder you had asked PM to watch, whose full path was sitting on the row the whole time. They all say it now, with the whole of a long one on hover, and the reader says it too rather than only offering a button that opens it.",
      "The Documents table stops wasting space. Each column is the width of what is in it, rather than reserving room for its worst case whether or not anything in it is that long — and all of that slack used to collect in one gap between a document's title and the buttons beside it. Names and paths still stop at a sensible width so one long one can't stretch a column; dates and sizes take exactly the room they need.",
      "The order you put the table in stays put. Sorting by size and then going to look at something else no longer drops you back to newest-first when you come back, and it survives closing and reopening PM.",
      "A new Source column, off until you switch it on from the Columns menu, says what PM is actually holding and what it is only pointing at: your vault, this device, each connected account, and, last of all, the ones PM can't reach just now and the ones whose original has been deleted. Anything with something wrong with it is marked in that column, so clicking the heading gathers a whole library's worth of trouble at one end — the bottom, or the top if you click again.",
      "Sorting by a column most documents have no answer for now keeps the blanks at the bottom whichever way you sort, which is what it was always meant to do. One direction was doing it and the other was moving them all to the top, burying every document that did have an answer.",
    ],
  },
  {
    version: "3.123.2-alpha",
    date: "2026-08-04",
    release: true,
    highlights: [
      "A little tidying when you first open this one. PM joins up files it can prove it was holding twice — the same Google Drive file reached two different ways — into one document that knows both of the places it lives. Your filing is merged rather than chosen between, and nothing is deleted to do it. It happens on the first launch and on the next Drive sync, and needs nothing from you. If you have never opened the Documents table's Columns menu, that table also starts from a slightly different set of columns than before; if you have ever ticked anything there, your choice is untouched.",
      "One file is one document, however many places it lives. If you own a file and a colleague has also shared it with you, PM used to hold it twice, with two filings and no way to tell the two apart. It now keeps one document living in two places, both still checked, so it stays readable as long as either one is there. It only ever joins two records when the provider's own id says they are the same file. The duplicate check has moved onto the Documents tab, runs quietly after any sync that brought something new in, shows where each copy came from, and lets you say “keep both”.",
      "Your documents say who wrote them, and keep saying it truthfully. Author, last editor, creation date and size now come through from Google Drive, OneDrive and your own machine — Word, PowerPoint, Excel and PDF files carry that inside the file itself. Created now means when the document was created, not when the copy on your disk was made. Every sync brings it all up to date, including the folder a file has moved into; PM used to ask once and never again.",
      "A Documents table you can shape. A Columns menu turns each column on or off, and every column sorts, with the rows that have no answer settling at the bottom either way. Two new facts join them: Updated, when the file itself last changed at its source, and Last synced, when PM last had something new to write down about it — which separates “nobody has edited this since March” from “this connector stopped working in March”.",
      "Sync you can stop, leave, and ask again from scratch. Stop indexing now bites inside the file listing rather than after a whole account. “Queued” stays put when you switch tabs. And a new Re-index everything reads every file in an account again, for when PM and the account look like they have drifted apart — it asks first, deletes nothing, and leaves the files PM already has alone.",
      "Re-tagging your library is something you can walk away from. Both halves show a real progress bar, and leaving the Teach tab and coming back rejoins the pass where it is. The vocabulary step is a model call, and its result is kept now rather than thrown away. A pass that ends says so, however it ended, and a second re-tag over a first is refused.",
      "Everything about where your data lives is in one place, and the export says what it actually writes. Export asks whether you want everything or just your documents, plain or encrypted, with a sentence for each combination — including how private the result is. The archive now carries the vault key file, your entity rules and every cloud pointer it had been leaving out, which is what makes it restorable at all.",
      "Backups show their work: a progress bar under the button you pressed, a first stage that shimmers instead of sitting at 0%, backups listed by date and time rather than a 68-character filename, and a failure banner that can be dismissed — when it was a failure at all, which most of the time it was not.",
      "A missing library is reported, never quietly re-created. If PM's store has been deleted or moved from outside PM, it stops, names the file, and tells you it has deleted and re-created nothing. It used to start over silently and open looking perfectly normal with nothing in it.",
      "Things you can see: switches with a visible edge and a visible off state at every theme, mode, accent and contrast level, checked by PM's own contrast audit so they cannot fade again; buttons that dim once instead of twice; a calendar that opens on an ordinary Monday-to-Sunday week and keeps swiping past the first day; and a rebuild list that keeps every file in the pass and survives the rebuild finishing.",
      "Under the hood, approving a large pile of documents no longer locks up the app, three libraries PM builds on took security updates, and a pre-release review closed several faults in the work above before any of it reached you — files in a connected account reported as missing when they were fine, a duplicate merge undoing your filing on the next launch, and an edit to a tracked file skipped for good if PM could not read it the first time.",
    ],
  },
  {
    version: "3.123.1-alpha",
    date: "2026-08-04",
    highlights: [
      "Files in a connected account are no longer reported as missing when they are perfectly fine. PM sometimes moves a file's record from one part of an account to another — a file you own that turned up inside a folder shared with you, or one indexed years ago under an older naming scheme. It was moving the record but not the note of where the file actually lives, so the part of PM that checks that account looked for something that was no longer there and concluded the file had been deleted. The file then showed as gone, refused to open, and nothing could bring it back: every later check repeated the same mistake. This was reachable straight from the new Re-index everything button, which is exactly the case a full re-read of an account was meant to help with.",
      "Your filing survives PM tidying up duplicates on its own. When PM joins two records of one file together, the project, labels and importance from both are merged onto the one that remains. That merge was being written to PM's own store but not to the portable copy in your vault — and moments later, on the same launch, PM reads that copy back as the truth. It was quietly undoing the merge, and because the other record had already gone there was nothing left to recover it from. A document you had filed and approved could reappear in Review, back in Unsorted.",
      "Keep both, and removing one side of a pair, now stick when you leave the tab. Your decision was being remembered only by the screen you were looking at, so the Documents tab came back offering the same pair again, with no sign you had already settled it. A removed document could even come back as a card with working buttons, for a document that no longer existed.",
      "An edit to a file in a tracked folder is never skipped because PM could not read it the first time. If a file was still open in the app that had just saved it, PM noted the file as seen before it had actually read the new contents — so it never looked again, and went on serving the old version in search and in answers until you happened to edit the file a second time.",
      "Re-index everything shows its progress the moment you press it, rather than after the listing finishes — which on a large account was minutes of a screen that looked like nothing had happened. It is also refused cleanly while a Rebuild is running: it used to discard the account's place-marker first and fail afterwards, which quietly turned the next ordinary background check into a full re-read of the whole account that nobody had asked for.",
      "The export screen now describes the file it actually writes. A plain export was said to open only on this machine — true when it held your documents and PM's store, but it now also carries the settings file a restore needs, so on a vault you protect with a passphrase it opens anywhere that passphrase does. It says that plainly, because it is the sentence you read while deciding where to put the file.",
    ],
  },
  {
    version: "3.123.0-alpha",
    date: "2026-08-04",
    highlights: [
      "Sync now on a Drive or OneDrive account has grown a second option: Re-index everything. The everyday button asks the provider what has changed since last time, which is one quick request and almost always the right answer. The new one reads every file in the account again — for when something looks out of date, or you suspect PM and the account have drifted apart. A change feed only ever reports what it noticed at the time, and there was previously no way to ask for a fresh look without disconnecting the account and starting over.",
      "It sits behind the small arrow next to Sync now, and asks before it starts, because on a large account it takes a while and uses bandwidth. Nothing is deleted and nothing you have filed is lost — files PM already has are recognised and left alone, so it is mostly listing requests rather than downloading your documents again. It also refreshes what the provider says about each file it sees: author, when it was created, how big it is, and which folder it lives in.",
    ],
  },
  {
    version: "3.122.3-alpha",
    date: "2026-08-04",
    highlights: [
      "Press Sync now on a second account while the first is still indexing and the “Queued” label now stays put when you switch tabs or close Settings. The sync itself was always safe — PM had noted it down and did run it — but the label was remembered only by the screen you were looking at, so leaving and coming back showed “Sync now” again, as though your request had been dropped. It is now read back from the sync itself, so both agree.",
      "It also says so when a background refresh has folded your request into a sweep of every account, which is what happens if the quarter-hourly check comes round mid-index. That used to leave nothing on screen at all, because the account you named was no longer what was waiting — a whole pass over everything was.",
    ],
  },
  {
    version: "3.122.2-alpha",
    date: "2026-08-04",
    highlights: [
      "Approving a big pile of documents no longer locks up the app. PM keeps one small index file describing everything it has indexed from a cloud drive, and it was rewriting that entire file — encrypting it from scratch — once for every single document you approved. Two hundred approvals meant two hundred full rewrites, all while holding the door to your library shut, which is why Windows started asking whether you wanted to close the program. It now writes that file once at the end, which is what it always meant. The same change speeds up deleting a label everywhere and renaming or merging a project, which were rewriting it per document too.",
    ],
  },
  {
    version: "3.122.1-alpha",
    date: "2026-08-04",
    highlights: [
      "Under-the-hood tidying: security updates to three of the libraries PM builds on, including the one that handles encryption inside the document-processing helper. Nothing about the app changes.",
    ],
  },
  {
    version: "3.122.0-alpha",
    date: "2026-08-03",
    highlights: [
      "Everything about where your data lives is now in one place, with the folder written above the button that opens it. The path used to sit as text in one section while “Open data folder” waited about forty lines away in another — and on a vault you had moved or joined, the two could point at different folders, so the button opened somewhere your documents were not. One section, one path, and the button goes where the text says.",
      "If your vault has been moved or shared, PM now also names its own settings folder on this account separately. They really are two places, and backing up the wrong one is the mistake worth avoiding.",
      "Export now asks what you actually want: everything or just your documents, plain or encrypted, with a sentence saying exactly what each combination produces. Plain means readable Markdown — PM’s own store stays encrypted inside the archive either way, which the old button never said. Encrypted writes the same .pmbackup file a backup does and needs a passphrase to open. An encrypted copy of the documents alone is the one thing PM won’t do, and it says so rather than quietly giving you something else: restoring one needs PM’s store and the vault’s key file, so an archive without them could never be restored.",
      "The export zip now carries what a backup carries. It had been leaving out the vault’s own key file, your entity rules and every cloud pointer — so unzipping it on another machine gave you a vault PM couldn’t open, and if it did open, one that had quietly lost things. On a moved or shared vault it was worse: the right database paired with a stale set of documents from the old folder.",
      "Sharing one vault between several accounts on the same PC is now folded away at the bottom, where a rarely-used feature belongs. On macOS and Linux it also says plainly what is missing: PM can open a shared vault that already exists there, but it can’t set one up, because finding the other accounts and granting them access is Windows-only. Opening an existing one stays where it was — for a Mac or Linux user it is the only way in there has ever been.",
    ],
  },
  {
    version: "3.121.0-alpha",
    date: "2026-08-03",
    highlights: [
      "One file in a connected account is now one document, however many ways you can reach it. If you own a file and a colleague has also shared it with you, or it sits in a shared drive that someone shared with you directly, PM used to hold it twice — two rows, two filings, two of everything, and no way to tell them apart on screen. It now recognises them as the same file and keeps one document that lives in two places. Nothing is deleted to do it: both places go on being checked, so the document stays readable as long as either one is still there.",
      "Copies you already have are joined up the next time PM opens, and whichever of the two you had filed is the filing that survives — projects, labels and importance are merged rather than picked between. Where a file lives now shows under it, in the reader and in the duplicate check, so “two places” is something you can look at rather than take on trust.",
      "This does not guess. PM joins two records only when the provider’s own id says they are the same file; two documents that merely look alike are still shown to you to decide on, exactly as before. Files in two different services stay separate — there is no shared id to go on, and inventing one would eventually merge two documents that were never the same.",
      "The duplicate check has moved out of Settings and onto the Documents tab, beside Rebuild. There is no longer anything to switch on: it checks in the background after any sync or import that actually brought something new in, and the button tells you what it found. Checking the whole library is still one click away for when you want everything compared against everything.",
    ],
  },
  {
    version: "3.120.0-alpha",
    date: "2026-08-02",
    highlights: [
      "Groundwork for the duplicate work: a document can now know about every place its file lives, rather than being one place. Nothing looks different yet — every document has exactly one location today, and the change that starts joining them up is next — but the rule underneath is worth knowing: a document stays readable as long as any one of its copies is still there. Lose the Drive copy of a file you also keep in a tracked folder and PM opens the folder copy instead of telling you the file is gone.",
      "A knock-on you may notice sooner: an expired Google or Microsoft sign-in used to grey out every file from that account, including ones PM could still reach perfectly well from somewhere else. It now only greys out the copies it genuinely can't reach.",
      "Nothing is deleted to resolve a duplicate, ever — that was the decision behind the design, and this is the shape that makes it possible. Two copies means one document with two records of where it is, and both keep being checked.",
    ],
  },
  {
    version: "3.119.0-alpha",
    date: "2026-08-02",
    highlights: [
      "Your own documents now say who wrote them. A Word file, a PowerPoint, a spreadsheet or a PDF carries its author, its last editor and the date it was created inside the file itself — PM reads that now, so files from a folder on your computer fill in the same columns as files from Google Drive instead of a row of “Unknown”. Files you drag in are covered too. It applies as each file is indexed, so existing files fill in the next time they change; anything you add from now on arrives complete.",
      "Created now means when the document was created, not when the copy on your disk was made. For anything you were emailed, downloaded, or restored from a backup those are wildly different dates, and only one of them is the one you meant. Where a file says nothing about itself, the date on disk still stands.",
      "Plain text, Markdown and web pages are unaffected — there is nowhere in those formats for an author to be written down, so PM doesn’t go looking, and they stay as quick to index as they were.",
    ],
  },
  {
    version: "3.118.0-alpha",
    date: "2026-08-02",
    highlights: [
      "The Columns menu closes when you click away from it. It was the last panel in PM that didn't, so picking your columns and then clicking anywhere else left it sitting open over the table. It now closes on a click outside, on Escape, and hands focus back to the button you opened it with.",
      "Every column sorts, including the four that describe the source. Author, Modified by, Created, Updated and Size were deliberately left unsortable, on the reasoning that ordering by a column reading “Unknown” for most rows would just pile the Unknowns at one end. That was a fair worry and it is answered directly instead: rows with no answer always sort to the bottom, whichever way the arrow points, so sorting by author never buries the files that have one.",
      "Two more things a document can tell you: Updated — when the file itself last changed at Google Drive, OneDrive or on disk — and Last synced, when PM last had something new to write down about it. Between them they separate “nobody has edited this since March” from “this connector stopped working in March”, which until now looked identical from the outside. Ingested now shows the time as well as the date.",
      "What each Depth starts with has been rethought. Minimal shows the project; Standard adds importance, author and size; Power adds the chunk count. Everything else waits until you turn it on. If you have ever opened the Columns menu and ticked anything, your table is untouched — your choice is yours until you press “Reset to depth”. If you never have, the table follows the new starting set: at Power that means Ingested steps back and Author and Size step in.",
      "Table headings are no longer shouted in capitals.",
    ],
  },
  {
    version: "3.117.2-alpha",
    date: "2026-08-02",
    highlights: [
      "What a document’s source says about it now stays true. PM asked Google Drive and OneDrive for the author, the last editor, the creation date and the size the first time it ever saw a file — and then never asked again. Rename the file, resize it, move it to another folder, hand it to a colleague to edit, and PM went on reporting whatever it had been told months earlier. Every sync now brings all of that up to date, including the folder a file has been moved into. It costs nothing when nothing has changed: a file that nobody has touched is recognised as unchanged and left alone, so the fifteen-minute check is no busier than it was.",
      "Something PM has been told is never forgotten just because the source goes quiet. Drive genuinely stops reporting an owner once a file moves into a shared drive, and Google’s own documents have no file size at all — so PM keeps the last thing it was actually told rather than blanking the field. “Unknown” means nobody ever said, not that PM lost it.",
      "OneDrive accounts set to sync particular folders now record which folder each file is in. PM has always read that from Microsoft’s reply, but for folder-scoped accounts it never asked for it — so those files showed no folder anywhere in PM, while accounts syncing the whole drive were fine. They fill in on the next sync.",
      "Rebuilding no longer wipes those four facts off your cloud files. Rebuild works from your vault, and your vault has never carried them, so it was writing “nothing” over them on every pass — the same bug the file-size field had already been fixed for, which the newer fields hadn’t inherited.",
      "PM also now records when it last had something new to write down about each document, which is the difference between “nobody has edited this since March” and “this connector stopped working in March”. It appears in the Documents table in the next update.",
    ],
  },
  {
    version: "3.117.1-alpha",
    date: "2026-08-02",
    highlights: [
      "Leaving the Teach tab while PM is choosing your tag vocabulary no longer leaves it stuck. Coming back part-way through showed a bar that never moved and every button greyed out, for as long as you stayed on the tab. The pass itself was fine — usually it had already finished — but the first half of a re-tag ended without ever saying so, and the screen was waiting to be told. Both halves now announce that they have stopped, whatever stopped them, so the tab can no longer be left waiting on a pass that is over.",
      "A re-tag that fails while you are on another tab now says what went wrong. It used to go quiet and look exactly like one that had never been started.",
      "Labelling shows its count from the first moment instead of after the first batch comes back. PM already knew it was about to label 165 documents; the bar simply wasn't being told until the first answers arrived, so the longest single wait in the pass was the one part with nothing to watch.",
      'The finished line counts what you will actually be asked to review. It counted every document the model answered for, so a well-tagged library was told "165 documents changed" above a list of three. It now also says so when a pass changed nothing at all, rather than leaving an empty list to speak for itself.',
    ],
  },
  {
    version: "3.117.0-alpha",
    date: "2026-08-02",
    highlights: [
      "PM now imports what the source knows about a document: who wrote it, who changed it last, when it was created there, and how big it is. Google Drive and OneDrive were never asked for any of it — the information was always there and PM simply wasn’t requesting it. Local folders contribute what a filesystem can (created and size); where a source has no answer, the field reads “Unknown” rather than going blank or quietly crediting you.",
      "The duplicate check now shows all of it side by side. That is the screen that motivated this: two copies of one file with the same title and the same date give you nothing to choose between them, and now the author, the last editor, the creation date and the size sit on each card, lined up so you can read across.",
      "You can choose which columns the Documents table shows. A “Columns” menu above the table turns each one on or off, including the four new ones. Your display depth still decides where it starts — so if you never open the menu, the table looks exactly as it did — and “Reset to depth” hands it back whenever you want.",
      "Opening a document in the reader shows the same four facts under its title.",
    ],
  },
  {
    version: "3.116.3-alpha",
    date: "2026-08-02",
    highlights: [
      "One Google Drive file no longer turns into two documents. If someone shares a folder with you and one of the files inside it is a file you own, PM was indexing it twice — once as your own file, once as part of the shared folder — and then correctly reporting the pair in the duplicate check. The duplicate check was right; the source was wrong. Your own files are now left to the part of the sync that already handles them, and a file you don’t own inside a shared folder carries on being indexed exactly as before, which is the whole point of syncing shared folders.",
      "The duplicate rows you already have are cleared up for you, not left behind. The next Drive sync merges each pair into a single document rather than deleting one and hoping. Nothing you set is lost: if you had confirmed the filing on one of the two, that one’s project and importance win; otherwise whatever is already filed stays put and any gap is filled from the other. Labels are combined, and a project that doesn’t end up as the document’s home is kept as a linked project — so the file is still findable everywhere you had put it.",
    ],
  },
  {
    version: "3.116.2-alpha",
    date: "2026-08-02",
    highlights: [
      "“Stop indexing” now stops when you press it. PM asks a connected account for its file list before it indexes anything, and a Stop could only take effect between accounts — so pressing it while a big Drive was still being listed meant waiting for that whole account, every shared drive and every shared folder included, before anything happened. The button now takes effect inside the listing itself, so a Stop lands in seconds. Nothing about that is riskier than before: a run that was interrupted knows it only saw part of your account, so it never treats the files it hadn’t reached yet as deleted, and it never records that it got further than it did — the next sync simply picks the listing back up.",
      "Leaving the Connectors tab mid-stop and coming back now shows what you left. The progress bar came back from where the sync really was, but the Stop button came back from nothing, so it read “Stop indexing” again next to a bar that had already been stopped — two parts of one screen disagreeing about the same run. Whether a stop has been asked for is now something the sync itself knows and the screen simply reflects, so the two can’t drift apart.",
    ],
  },
  {
    version: "3.116.1-alpha",
    date: "2026-08-02",
    highlights: [
      "Switches now have a visible edge, at every theme and contrast setting. An off switch used to be a dot floating on what looked like empty page, and the outline added to fix that was drawn in a grey only 1.4–1.8 times lighter than the page behind it — measurably too faint to see, which is why it looked like nothing had changed. The outline is now drawn in a tone that is guaranteed to stand out against both the switch and the page in every system, mode, accent and contrast level PM offers, and the app's own contrast audit now checks it, so it can't quietly fade again.",
      "Swiping sideways across the calendar keeps working past the first day. The gesture stepped the calendar once and then went dead: the day columns are rebuilt every time the view moves, and the swipe was listening on the very columns it had just replaced. It now follows your pointer rather than the columns, so a long swipe keeps moving — and it works the same from a trackpad, a tilt wheel or a side wheel.",
    ],
  },
  {
    version: "3.116.0-alpha",
    date: "2026-08-01",
    highlights: [
      'The duplicate check now tells you which is which. Two copies of one file used to render identically — same title, the same "indexed from a connected account" sentence, the same project, the same date — on the one screen that asks you to delete one of them. Each side now says where it actually came from: which Google account, or that it was shared with you, or which shared drive, or OneDrive, or this device — plus the folder it sits in, and the time it was added rather than just the day. All of that was already known and simply wasn\'t being shown.',
      'You can now say "keep both", and PM will remember. Until now the only options were to delete one or to be asked about the pair again on every single check — the report is worked out from scratch each time and remembered nothing, which is why running a check after a rebuild looked like it had found the same duplicates all over again. It had; it just had no way to know you\'d already decided. PM always says how many pairs are hidden and offers them back, so nothing is quietly dropped from the list.',
      "PM now says that it compares what's inside a document, not what it's called. A file you renamed still matches its original, which is correct and deliberate — but with nothing explaining it, a correct match read as a mistake.",
    ],
  },
  {
    version: "3.115.0-alpha",
    date: "2026-08-01",
    highlights: [
      "Re-tagging your library now has a proper progress bar, and you can walk away from it. Both halves of a re-tag — choosing the vocabulary and labelling every document — show the same progress bar the rest of PM uses, with the count and, if your display depth is set high enough, a percentage and elapsed time. Leaving the Teach tab and coming back now rejoins the pass exactly where it is, instead of showing you nothing and leaving you unsure whether it was still going. It always was still going; PM had simply stopped listening to it.",
      "Choosing a vocabulary no longer throws away what you paid for. That step is a model call, and leaving the tab while it ran discarded the result — so you'd come back to nothing and have to run it again. It's kept now, and waiting for you.",
      'The "Reading your library" message is no longer written on a greyed-out button. Using a disabled button as the status readout put the only word about what PM was doing at about a seventh of the contrast it needed, out of reach of the keyboard, and invisible to a screen reader.',
      "PM refuses to start a second re-tag over a first. Starting a pass wipes any proposals already staged, and switching tabs used to reset the safeguard that prevented it — so a second pass could quietly destroy the half-reviewed results of the first. The proposals list also stays out of the way while a pass is running, rather than offering to apply a set that is still changing underneath you.",
    ],
  },
  {
    version: "3.114.14-alpha",
    date: "2026-08-01",
    highlights: [
      'Week view opens on a proper Monday-to-Sunday week again. Leaving the Calendar tab and coming back set the first column to today, every time, so from Wednesday onwards you were looking at a week that began mid-week — with no way to get the ordinary grid back short of pressing Today. It opens on the Monday of the current week now, which is the same shape Today gives you. If you would rather it picked up exactly where you left it, start day and all, that is what "open where I left off" in Settings has always been for, and it now covers the week\'s shape too.',
      "The Calendar's sideways scroll works. Nudging the grid horizontally to shift a day at a time was announced back in 3.88 for both the Calendar tab and Focus, but on the Calendar tab it was never actually connected to anything — only the Focus grid has ever responded. It is connected now, and it no longer depends on the grid happening to be on screen at the instant the tab loads, which could leave the gesture dead for the rest of the session. Mice that report scrolling in lines rather than pixels — where a whole notch counted for a single pixel against a threshold of 55 — are handled too.",
      'Opening the Calendar tab no longer overwrites where you left off. If you have "open where I left off" turned on, simply visiting the tab used to record today as the place you\'d left, so it could only ever take you back to your last visit rather than the day you actually navigated to.',
    ],
  },
  {
    version: "3.114.13-alpha",
    date: "2026-08-01",
    highlights: [
      'The rebuild Activity list keeps every file in the pass, and scrolls. It used to hold only the last 50 — so a 161-file rebuild had thrown away two thirds of itself while you were still watching it, under a line saying "showing the most recent files". It now keeps the whole run for any normal library, in a box about fifteen rows tall that scrolls, instead of running down the page and pushing the document table out of sight.',
      "The list also survives the rebuild finishing. Until now the moment a rebuild ended, coming back to the tab showed you nothing at all — the files it had just built were dropped. They stay, so you can scroll back through what was done.",
      "Folding Activity away now sticks. Leaving the Documents tab unmounts it, so the fold sprang back open every time you returned; it remembers per machine now.",
      "\"Done — 161 ingested\" stops following you around. That line was replayed from the last rebuild every time you opened the tab, and only starting another rebuild — or restarting PM — ever cleared it. It now bows out once you've seen it, and there's an × to clear the whole card, list and all.",
      "Dropping files in no longer shows the previous rebuild's file list as if it belonged to the new import.",
    ],
  },
  {
    version: "3.114.12-alpha",
    date: "2026-08-01",
    highlights: [
      "Backing up now shows its progress where you pressed the button. The progress bar existed and worked — it just rendered near the top of the tab, about five sections above the \"Back up now\" you'd actually clicked, so on any normal window the feedback was off-screen and the backup looked like it had done nothing. There's now a bar under the button too, and it only appears for the destination you started.",
      "The first stage of a backup no longer looks stuck. Preparing the snapshot is a single database operation that can't report a percentage, so the bar sat frozen at 0% for what is often the longest part of a backup. It shimmers instead, like every other step PM can't measure. Verifying a restore did the same thing and is fixed with it.",
      "Your backups say when they were taken. The list showed the raw file name, which is 68 characters with the date on the END — so the part that got cut off was exactly the date you were looking for. Each row now leads with the date and time, with the size and full file name underneath. Saving a backup to this computer suggests a dated file name too, instead of offering to overwrite the last one.",
      '"Backed up, but 1 destination failed" stops haunting you. That message was replayed from the last run every time you opened the tab, and only another backup could ever replace it — so if you fixed the problem yourself, PM kept telling you about it forever. It now has a Dismiss button that actually sticks.',
      "…and most of the time it was never a failure at all. If PM couldn't tidy up your older backups to respect \"keep the last N\", that was being reported as the destination failing, even though your backup had uploaded perfectly. Those are now separate sentences, and the tidying one clears itself: delete the extra backups in Drive and the message is gone next time you look. If PM can't see the destination to check, it keeps the message rather than guessing.",
      "Opening the Backup tab during a backup no longer shows the previous run's failures underneath the live progress bar.",
    ],
  },
  {
    version: "3.114.11-alpha",
    date: "2026-08-01",
    highlights: [
      "Switches you can actually see when they're off. The little circle inside a switch was being painted in a colour only ever meant to sit on the accent fill — which is where it sits when the switch is ON. Turned off, it moved onto a different background where that colour means nothing, and on PM's default look it came out as exactly the page colour: an invisible circle in a nearly invisible pill. Every switch in Settings did this. Off now uses a colour with a guaranteed readability floor, so the off state reads clearly in every theme, mode and accent.",
      "Switches now have an outline, so you can see the switch and not just the circle. The pill behind the circle was filled but never edged, and its fill sits only one step away from the panel it lies on — enough to read as a slightly raised patch, nowhere near enough to read as the edge of a control. The outline uses PM's strong edge colour, which means it goes darker on a light background and lighter on a dark one, and it firms up further when you turn Contrast to High — where before, switches were the one control High contrast did nothing at all for.",
      "Buttons that are unavailable now dim once instead of twice. A disabled button was both recoloured and made see-through, and the two stacked — the label and the button faded into the background together, ending up practically invisible rather than merely inert. The colour change alone is a big step, and it's what PM's design rules specify.",
    ],
  },
  {
    version: "3.114.10-alpha",
    date: "2026-08-01",
    highlights: [
      'If PM\'s library file ever goes missing, PM now says so instead of quietly starting over. Until now, a store that had been deleted or moved from outside PM — by a disk cleanup, a restore that went sideways, a stray drag — was simply re-created empty on the next launch, and PM opened looking perfectly normal with nothing in it. Everything kept outside that one file, like your themes and your saved keys, survived, which made it look even more like your documents had vanished. PM now stops, names the missing file, and tells you plainly that it hasn\'t deleted or re-created anything, so you can put the file back or restore a backup first. If you\'d rather carry on, "Start a new empty store" does exactly that and deletes nothing — your notes, your vault settings and your sign-ins all stay put, so a recovered file still opens and Rebuild can put your library back from the notes in your vault. The old "Start fresh" is refused for this, because there is nothing broken to delete and it would take your notes with it.',
      '"Start fresh" now says that it deletes your notes too. It always did — the wording only mentioned the vault and promised your keys and sign-ins were kept, which read as though the notes were safe.',
      "The Developer tab tells you when it can't read the database, rather than showing you an empty table list that looks exactly like a database with nothing in it.",
    ],
  },
  {
    version: "3.114.9-alpha",
    release: true,
    date: "2026-08-01",
    highlights: [
      "PM 3.114.9 rolls up everything since the last release into one update. Here's the tour at a glance — every line below has its full story in the entries that follow. Unlike the last few releases, this one does a little work when you first open it: PM spends about a minute reinstalling its document engine so every component in it is checked against a known fingerprint before it runs, and your conversations move into their own chats folder inside the vault — a plain rename, nothing re-encrypted, nothing re-indexed. If older versions left deleted photos or spreadsheets behind, PM offers once to clear them out and deletes nothing until you say so. Your files, vault and settings are untouched by all of it. If you use photo text recognition or the memory map's detailed layout, one may show as not installed afterwards — re-adding it from Settings → Storage is quick.",
      "A document can belong to more than one project. The Project field is a list you build like the To: line of an email. The first is the primary project — the one that owns the document, counts it as activity and places it on the Map — and the rest are links. You can see which is which wherever a document appears, and change it on the spot. A project's chat can now answer from documents linked into it, not just the ones filed there. Existing vaults carry over untouched.",
      "Your tags are yours to edit, and PM stops inventing them. Teach → Tags lists every label with how many documents carry it: rename one everywhere, fold near-duplicates together (tax and taxes, chair and chairs), or take one off every document. You can also re-tag your whole library from a single vocabulary chosen with everything in view. That last part fixes something quietly broken — tags proposed a few documents at a time produced labels like 'ammun' and 'placement', fair descriptions of one file and useless as tags, because a tag that lands on one document groups nothing. Nothing is written until you approve both the vocabulary and the per-document before-and-after. You can also point a chat at a tag: type @ in the message box, pick one, and that single message searches it.",
      "Projects you can merge, delete, and re-file into. Merge one project into another and everything it holds moves across before the old one is deleted. Delete a project and choose separately what becomes of its files, its chats and its name. Delete a single document from wherever you found it. Each one counts up exactly what it holds and asks you to type the name first, because none of it can be undone from inside PM. \"Part of\" has been removed — it hid a project's own status behind a parent's name, and merging does honestly what it was really being used for.",
      "Sorting a new import happens as the files land. Each file appears in Documents and in Review the moment it's stored, already carrying a proposed project, importance and tags — so you can approve the first handful while the rest are still arriving, instead of waiting for the whole sync. Every file gets a suggestion however it reached PM, and PM never asks twice about one it has already suggested for. PM can also look through your library for documents you have twice, comparing both the opening text and what each document is about, so the same report saved as a Word file and a PDF still matches.",
      "Every repeating calendar event now shows every time it happens. PM was treating a whole repeating series as a single event, so a weekly meeting appeared exactly once — your Month, Week and Year views and Upcoming on Focus will suddenly look a lot fuller, which is the honest picture. Editing a single occurrence no longer makes it vanish, and a calendar sync that doesn't finish now adds what it found and removes nothing.",
      "Your connected accounts keep themselves up to date, and a half-finished sync stops costing you files. Google Drive, OneDrive and local folders are now checked when you open PM and every 15 minutes after, the way your calendar already was, with \"Shared with me\" on its own hourly rhythm. A sync that only partly succeeded no longer removes anything: one unreadable folder, one file OneDrive won't hand over, or a drive that went away no longer takes the whole account down or quietly shortens your library. And a problem with the document engine no longer makes files disappear without a word — PM used to treat everything it couldn't read as unreadable forever and move its place-marker past it, so those files were never offered again.",
      "\"Remove PM data\" now really removes everything, and says what it couldn't. It reaches the caches, old installers and sandbox profiles PM had been leaving in folders you'd never think to look in, the Mac and Linux leftovers it never touched, and a vault you had moved elsewhere. It refuses while a backup is uploading, won't touch a vault another account owns, tells you when erasing a shared vault takes it away from other accounts too, and lists what it left behind with the exact path instead of claiming everything is gone.",
      "Your vault is harder to tamper with and safer to interrupt. Notes, chat transcripts and saved photos are now written and swapped into place in one step, so a crash mid-save leaves you the old version rather than half a file. A settings file edited to claim your notes aren't encrypted is refused rather than re-signed. Changing the passphrase on a vault shared between two accounts asks you to confirm you're taking it over. And an operation that can't read part of your vault — a passphrase change, an export, a backup — now stops and says so instead of finishing and reporting success. You can also keep typing while PM is still answering: press Enter and your message waits its turn, up to three at a time.",
      "Reading PM is easier, and it speaks properly now. Every status colour clears the readability floor in light mode — Take a look was as low as 3.1 against a required 4.5 — text you're meant to read is out of PM's faintest grey, every dialog has a name a screen reader can announce, and errors and warnings are announced rather than failing in silence. Reduced motion is honoured everywhere, including your own setting rather than only your system's.",
      "Licences, properly, and a smaller install. PM's own licence now installs alongside the app, the credits file covers the interface's open-source work and its typefaces too, and every local model PM suggests says what its weights are licensed under — with the seven under publisher terms showing you those terms before the download rather than after. PM also got smaller: it had been installing 82 MB of debugging files it never needed, so the bundled Python drops from 150 MB to 69 MB.",
    ],
  },
  {
    version: "3.114.8-alpha",
    date: "2026-08-01",
    highlights: [
      "A problem with PM's document engine no longer makes cloud files disappear from your library without a word. If the engine broke — a half-finished install, a missing component — PM treated every file it couldn't read as a file it would never be able to read, marked it skipped, and moved its place-marker past it. Google Drive and OneDrive only ever report what has changed since that marker, so those files were never offered again: the sync said \"finished, 0 indexed, 3,000 skipped\", the account looked perfectly healthy, and the only way back was to disconnect the account and add it again. PM now tells the two cases apart — a file the engine has read and refused is skipped as before, but an engine that is simply broken holds the marker where it is, so everything is picked up again once it's repaired.",
      "On Windows, removing only your app preferences no longer reports that nothing was removed. It said so every time — the preferences were genuinely gone, and the message arrived after they went — because the only part of that job Windows leaves to PM is done by the app itself, and PM was counting the other part. It now says what it did, and names the browser-side store it can't remove while running, which the uninstaller takes care of when you remove PM itself.",
      "On a Mac, dismissing the keychain prompt at startup no longer leaves you stuck. PM asks the keychain once per launch, deliberately — asking again for every part of the app that needs a key is how you end up with a dozen prompts. But that limit also applied to the Retry button on the \"can't read your saved keys\" screen, so once you'd said no, Retry could never succeed no matter how many times you pressed it, and quitting PM was the only way out. Retry now genuinely asks again. Nothing else asks again, so the flood stays fixed.",
      "Erasing a shared vault now says who else loses it. Deleting a vault you own also deletes it for every account you had linked to it — that is what it means and it hasn't changed, but the summary described it as your own data and named nobody, even though the list of linked accounts was sitting in the folder being deleted. It now names them.",
      'If removing your data fails part-way, PM stops telling you nothing happened. Some refusals — a backup still uploading, a database file held open by antivirus — arrive after PM has already cleared this window\'s own preferences, so "nothing was deleted" was true of your files and untrue of the app you were looking at. It now says your data is untouched, tells you the preferences went anyway, and asks you to restart before trying again instead of leaving you one click from repeating it.',
      "The date picker opens where you can see it inside a pinboard folder set to Overlay. It was being drawn behind the panel, so clicking a date field appeared to do nothing at all and the next click closed it again. Typing the date always worked; now the calendar does too.",
    ],
  },
  {
    version: "3.114.7-alpha",
    date: "2026-08-01",
    highlights: [
      "More under-the-hood tidying, again with nothing to see. The Backup settings screen was one very large piece of code holding every destination, the schedule, restoring and the passphrase all at once. It is now split by what each part does, which is what makes the next fix to any of them a small change rather than a careful one.",
    ],
  },
  {
    version: "3.114.6-alpha",
    date: "2026-08-01",
    highlights: [
      "Under-the-hood tidying, with nothing to see. PM's largest source file had grown to eleven thousand lines holding every one of its two hundred-odd internal operations, which made it hard to find anything and easy for the same mistake to be made twice in two corners of it. It is now seventeen files organised by what they do. Nothing moved but the code itself.",
    ],
  },
  {
    version: "3.114.5-alpha",
    date: "2026-08-01",
    highlights: [
      "Removing PM's data now refuses while a backup or restore is running, rather than going ahead. It could otherwise finish uploading a complete, readable copy of your vault to Proton or Drive after the erase said it was done — a copy that opens on any machine with the passphrase you still know. PM waits for the backup instead, and tells you so; nothing is deleted when it refuses.",
      "The semantic map can no longer get stuck. If a layout pass failed in an unusual way it left a marker saying one was still running, so the map served its old positions with the spinner on forever, and installing or removing a component quietly stopped recomputing — until you restarted PM.",
      "On macOS and Linux, PM now cleans up after a sidecar it had to restart. Each restart left a dead process entry behind; on a machine where the sandbox never works, that was two per attempt.",
    ],
  },
  {
    version: "3.114.4-alpha",
    date: "2026-08-01",
    highlights: [
      "A tag stops being offered the moment nothing carries it any more. Deleting the last document with a particular tag used to leave that tag sitting in the pickers — and, more annoyingly, in what PM tells the model about your library, so it would cheerfully suggest the tag again — until some unrelated filing elsewhere happened to tidy it away.",
      "The count on the Review tab is no longer worked out by reading through your whole library. On a large library that was a visible pause every time the number refreshed.",
      "Fixed a way a failed filing pass could leave your files changed while PM's own record of them was rolled back. If PM couldn't write its notes file at the end of a Review commit — a full disk, an antivirus lock — it undid the database half and not the files half, so the two disagreed and the next re-index adopted the version the database had rejected.",
      "Filing a document is a little quicker, and noticeably so during a bulk re-tag, because PM no longer tidies its entire tag list once per document.",
    ],
  },
  {
    version: "3.114.3-alpha",
    date: "2026-08-01",
    highlights: [
      "Status colours are noticeably deeper in light mode. Due soon, Blocked, Quick win, Take a look and Part of were all below the readability floor as text on a light background — Take a look as low as 3.1 against a required 4.5 — and turning on High contrast did not help, because that setting never touched these colours at all. Every one of them now clears the floor on every background, in all three looks and in the colour-blind-safe palette. Amber moves the furthest, so warning notices read considerably stronger. Dark mode is unchanged.",
      "Text you are meant to read is no longer drawn in PM's faintest grey. That grey sits well below the readability floor, so around fifty places — empty states, file paths on the vault screens, counts, timestamps, the notes under settings — have moved up a step. The faint grey stays, but now only for genuine decoration: separators, placeholder dots, and disabled buttons.",
      "The last settings fields that were labelled only visually are now properly attached to their controls, so a screen reader reads the right name for each. Getting a preference wrong in Teach also says so out loud now; it used to fail completely silently.",
    ],
  },
  {
    version: "3.114.2-alpha",
    date: "2026-08-01",
    highlights: [
      "Forgetting your backup passphrase now asks first, and says what it costs. It was a single click that could not be undone — PM keeps no other copy, so every backup you already have becomes permanently unreadable — and it quietly switched automatic backups off at the same time without telling you. Disconnecting a backup destination now asks too, the way every other connector already did.",
      "Every notice and warning in PM now speaks with one voice, and all of them are announced to screen readers. Around forty-five of them had been written by hand, one at a time, which is why the same kind of warning came in five slightly different shades — and why most of them said nothing at all when something failed.",
      'Every dialog now has a name a screen reader can read out; twelve of them had none, so they announced as just "dialog". Settings and the command palette are now proper dialogs too, which means Escape closes them from anywhere rather than only while the search box has focus, and your place on the page is restored when they close.',
      "Buttons across the app now use one set of sizes. Many small buttons had been asking to be small and silently rendering at full size, so a number of toolbars and rows are a little tighter and more consistent than before.",
      "Settings labels are now properly attached to the controls they name, and a few switches were announcing different words than the ones printed beside them.",
    ],
  },
  {
    version: "3.114.1-alpha",
    date: "2026-08-01",
    highlights: [
      "PM now honours Reduced motion everywhere, and honours your own Reduced-motion setting as well as your system's. Several places still scrolled smoothly whatever you'd asked for — jumping to a chat message, the pinboard, the settings pane, and moving through months and years in the calendar. They're instant now if you've asked for that.",
      "Sizes in gigabytes are all measured the same way. One place counted a gigabyte as a billion bytes while everywhere else counted it the way your operating system does, so a model download's progress didn't match the size shown on its card. Small sizes also read properly now — a 96 KB component said \"0 MB\" before.",
      "Error and warning notices are now announced to screen readers. Most of them simply weren't, which meant that if something failed, nothing was said and the app looked idle — including on the screens for getting back into your vault, where it matters most.",
      "Under the hood, the parts PM had been re-typing by hand — notices, dialog framing, the label-and-control rows in Settings — are now single shared pieces. That's invisible today, but it's what stops the same notice being five slightly different colours and stops the next dialog shipping without a name a screen reader can read.",
    ],
  },
  {
    version: "3.114.0-alpha",
    date: "2026-08-01",
    highlights: [
      "Links in the briefing window work again. The little always-on-top briefing renders the same Markdown as the main window but never got the piece that opens a link in your browser, so every link in it was silently dead.",
      "If you share a vault between two accounts on one PC, changing the passphrase no longer quietly makes you its owner. PM now asks you to confirm you're taking over first, and records that it happened so it's visible rather than silent — and \"Make private\" is refused outright for a vault that isn't yours, because that one re-keys it to your account alone and moves it, with no way back in for anyone else.",
      "Your per-project milestone sort, the calendars you've hidden and the backup notices you've dismissed now live in PM's encrypted store instead of the browser's local storage. That means they're encrypted, they're removed when you remove PM's data, and — the part you'll actually notice — they survive a backup and restore, and moving to a new machine. Until now they quietly didn't.",
      "Error messages no longer print the whole address of something private. A calendar feed's secret URL or an upload session link could appear in full in an error you might screenshot or paste into a bug report; PM now keeps just the site name.",
      "A few quieter hardening fixes: a link in an imported document can no longer dodge PM's URL checks by leaving off the http:// part; PM won't send your OneDrive credentials to an address that isn't Microsoft's, even if OneDrive asks it to; a local AI address is re-checked at the moment PM calls it rather than only when you saved it; the text of your calendar events is no longer placed among PM's own instructions when it decides what a note is about; and reading a photo now checks it really is a photo first.",
    ],
  },
  {
    version: "3.113.5-alpha",
    date: "2026-07-31",
    highlights: [
      "An import no longer reports a clean run over files it never managed to open. Drop in a folder where one subfolder is locked — by another program, by a permission, by a drive that went away — and PM used to reach 100% and say nothing was skipped. It now says how many items it couldn't read.",
      "Changing your vault passphrase, or exporting everything as plain text, now stops rather than half-finishing if PM can't read part of the vault. A file it can't see would previously have been left behind under the old key, unopenable. It now refuses before touching anything and tells you what it couldn't read.",
      "A backup can no longer quietly leave your whole vault out of the archive. If PM couldn't read the vault folder it treated it as \"there isn't one\", packed the rest, verified it and called it done.",
      "Saving a photo now writes your copy of the original after the photo is safely indexed rather than before, so a failure part-way through no longer leaves an unreferenced file sitting in the vault. If the copy itself fails you're told, on that document.",
      "In the document reader, text that sits in the overlap between two passages is no longer shown twice. The passages PM searches deliberately overlap a little; the reader was drawing both. A side effect worth knowing: the alternating bands now cover unequal amounts of text, so band height was never — and is now visibly not — a guide to how big a passage is.",
    ],
  },
  {
    version: "3.113.4-alpha",
    date: "2026-07-31",
    highlights: [
      "Approve in the sorting review no longer files a document before its suggestion has arrived. If files landed while PM was still working through them, Approve and Approve all would file those rows with nothing filled in — straight to Unsorted, and out of the review queue for good. Approve all now tells you how many rows are ready and leaves the rest where they are, and a row still waiting says so.",
      "A file that arrived while the review screen was loading is no longer left out. It used to render with none of its details filled in and never got a suggestion at all — and editing one of those rows could blank the whole window.",
      "PM stops recording corrections you never made. Filing a document yourself was logged as though you had corrected an AI suggestion that was never offered. It now only records a correction when there was really something to correct. Existing entries are left alone: nothing about them says which were the invented ones, so removing them would be guesswork over your data.",
      "When PM suggests where a document belongs, it can see the tags you already use again. A change to how imports are batched had quietly closed that door, so on a fresh library the model kept inventing new tags instead of reusing yours. It now looks at everything still waiting to be reviewed, decides the vocabulary once for the whole import, and reuses it.",
      "If a screen ever fails, you get a card explaining it with a way back, instead of an empty window. PM's window has no system title bar — it draws its own — so a crash used to take the close button with it, leaving nothing to click.",
      "Smaller repairs: a message you typed and left behind can no longer reappear in the conversation you moved to; a milestone no longer writes an out-of-date name or date back over a newer one, and a date that fails to save visibly reverts; and scrolling to the bottom of an inner list now carries on scrolling the page behind it instead of stopping dead.",
    ],
  },
  {
    version: "3.113.3-alpha",
    date: "2026-07-31",
    highlights: [
      "Repeating calendar events now show every time they happen. PM was treating a whole repeating series as a single event, so a weekly meeting appeared exactly once in your agenda and once in the calendar — the other fifty-one were there in the mirror but never drawn. Your Month, Week and Year views and the Upcoming list on Focus will suddenly look a lot fuller, which is the honest picture.",
      "Editing just one occurrence of a repeating event no longer makes it vanish. If you changed the title or the room for a single Tuesday and left the time alone, that Tuesday disappeared from PM entirely. It now shows, with your edit. All-day repeating events also now land on the dates the feed actually names, on every machine — before, they could sit a day out.",
      "A calendar sync that doesn't finish no longer quietly shortens your calendar. If PM only got part of the way through fetching a calendar, it used to replace the whole mirror with whatever it had, so events silently went missing until the next full sync. Now it adds what it found, removes nothing, and the calendar's panel tells you the sync didn't finish.",
      "Cloud connectors cope with the awkward cases instead of giving up on the whole account. One folder in your Drive you're not allowed to open no longer marks the entire account unreachable, and a single file OneDrive will never hand over no longer leaves the account stuck on an error forever, never syncing again. Reconnecting after a long gap now also notices the files you deleted while PM wasn't looking, instead of leaving them in your library.",
      "Tracked folders on this device keep up with folder-level changes. Renaming or deleting a subfolder inside a folder you track is now picked up straight away, rather than leaving those files pointing at a path that no longer exists until the next scheduled scan. And if part of a tracked folder can't be read — a drive that unmounted, a permission you don't have — PM says so and removes nothing, instead of assuming everything under it was deleted.",
      "Background work now waits its turn during a scan of a folder on this device, the way it already did for Drive and OneDrive. That includes the scheduled backup and the tidy-up that removes files, so neither runs against a half-scanned picture.",
    ],
  },
  {
    version: "3.113.2-alpha",
    date: "2026-07-31",
    highlights: [
      "The numbered sources under a grounded answer now always point at the file they say they do. When PM found two useful passages in the same document, the [1], [2] markers in the answer could drift out of step with the list of sources beneath it — so a citation quietly named the wrong file. The numbering now works document by document, so the marker, the list and the file you open can't disagree.",
      "Fixed a way a Google Drive file shared with you could turn into two copies of itself after a re-index, and a way a filing change could quietly revert to a previous one if PM couldn't finish writing it down at the time. Nothing you file is lost to either now.",
      "In a very long conversation, one of your own questions could fall out of both PM's memory of the chat and its search — reachable by nothing. It now stays findable.",
      "The retrieval inspector now examines exactly the same set of passages your last answer was built from, instead of a slightly wider one.",
    ],
  },
  {
    version: "3.113.1-alpha",
    date: "2026-07-31",
    highlights: [
      "Under-the-hood tidying. Several parts of PM that quietly do the most important work — deciding how a document is cut into searchable pieces, packing your backup, and applying what a cloud sync found — now have automatic checks that would catch a mistake in them before it reached you. The one that matters most catches a change to how documents are split that forgets to tell PM to re-index: without it, PM would go on searching an old map of your library and never say so.",
      "The safety rule that stops an update from ever deleting or rewriting your existing data now reads each database instruction properly, rather than only the first line of each. Three ways of quietly overwriting rows that it used to miss are now refused outright.",
      "PM also now checks, on every change, that every screen is asking the app for something the app can actually do — a mismatch used to surface as an error the first time you opened one particular screen, sometimes a release later.",
    ],
  },
  {
    version: "3.113.0-alpha",
    date: "2026-07-31",
    highlights: [
      "Every local model PM suggests now says what its weights are licensed under, right on its row, with a link to the terms. Most are properly open — Apache-2.0 or MIT — but seven are not: Google's Gemma 2 and 3, Meta's Llama 3.1 and 3.2, and the largest Qwen 2.5 come with the publisher's own conditions attached.",
      "For those seven, PM now shows you the terms in plain language before the download starts, rather than after. Accept once and PM won't ask again for another model under the same licence; the open ones are never interrupted at all. PM doesn't fetch the weights itself — your own Ollama does — so this is PM telling you what you're agreeing to, not PM policing it.",
    ],
  },
  {
    version: "3.112.3-alpha",
    date: "2026-07-31",
    highlights: [
      "PM's photo text-recognition add-on now uses a smaller, read-only reader for iPhone photos. The previous one carried a 22 MB video *encoder* PM never had any use for, on licence terms that sat awkwardly beside PM's own. If you already had photo text recognition switched on, Settings → Storage will offer it to you again — adding it back takes a moment and downloads noticeably less than before.",
      "Under-the-hood tidying. Every Python package PM installs for the document features now has its licence recorded and checked, so a change in what those terms are can't slip by unnoticed. The file-header check covers four more kinds of file and no longer skips anything at all. And the security page now explains how PM stores your keys on each platform, rather than describing the Mac and leaving Windows and Linux to be guessed at.",
    ],
  },
  {
    version: "3.112.2-alpha",
    date: "2026-07-31",
    highlights: [
      "The Python runtime PM installs alongside itself is built on a lot of other open-source work — OpenSSL, SQLite and half a dozen others. Their licences now ship with it instead of being left out, and they come from the exact build PM uses, so they can't quietly stop matching what is actually inside.",
      "PM also got noticeably smaller along the way. It turned out to be installing 82 MB of debugging files that are of no use outside a developer's machine, and had been doing so for as long as it has bundled Python. Those are gone: the installed Python drops from 150 MB to 69 MB.",
    ],
  },
  {
    version: "3.112.1-alpha",
    date: "2026-07-31",
    highlights: [
      "PM is free software, and its licence now travels with it: a copy is installed alongside the app instead of living only in the source code. If you want to read it, it sits next to PM's program files.",
      "The credits file that ships with each release now covers the open-source work behind PM's interface too — including the four typefaces it uses — rather than only the parts written in Rust. Several of those licences ask for exactly that, so this was owed rather than optional.",
    ],
  },
  {
    version: "3.112.0-alpha",
    date: "2026-07-31",
    highlights: [
      "If you deleted photos or spreadsheets on an older version of PM, the files themselves were left behind in your vault. They don't appear anywhere, but a rebuild would bring them back as documents you thought you'd removed. PM will now offer, once, to clear them out — it shows you exactly what it found and deletes nothing until you say so.",
      "That prompt only appears if you actually have some, and it won't come back once you've answered either way. Please back up your vault before approving: deleting them is permanent and can't be undone from inside PM.",
    ],
  },
  {
    version: "3.111.11-alpha",
    date: "2026-07-31",
    highlights: [
      "Under-the-hood tidying: the machines that build and test PM have moved to the current long-term-support version of their toolchain. The previous one stopped receiving security updates in April, and PM is now built and tested on the same version it is developed on — which had quietly drifted apart.",
    ],
  },
  {
    version: "3.111.10-alpha",
    date: "2026-07-31",
    highlights: [
      "PM's document engine now checks every piece of software it downloads against a known fingerprint before installing it. This is the part of PM that opens your PDFs, documents, spreadsheets and photos, so it is the part most worth protecting: previously it pinned the handful of main components by name and version, and accepted whatever else came along with them.",
      "The first time you open PM after this update, it will spend a minute reinstalling that engine so everything in it is fingerprint-checked. Nothing you have added is touched — your files, vault and settings are all untouched.",
      "If you use photo text recognition or the memory map's detailed layout, those are covered too, and each is now installed in a way that can't disturb the rest of the engine. You may see one of them listed as not installed after this update — reinstalling from Settings → Storage is quick, and usually near-instant.",
    ],
  },
  {
    version: "3.111.9-alpha",
    date: "2026-07-31",
    highlights: [
      "A release could go out with no Mac installer in it. If the packaging step failed to produce the .dmg file, PM shrugged and published anyway — leaving Mac users at a download page with nothing they could install. That is now a hard stop, and PM builds and checks the real Mac app on every change, the same way it already does for Linux.",
      "Under-the-hood tidying: the release process now always builds exactly the version you asked for. Re-running a release by hand could quietly build the newest code instead of the version named — the two are usually the same, but 'usually' is not good enough for something people download.",
      "The tools PM's build machines download are now checked against a known fingerprint before they run, and two build queues can no longer trip over each other.",
    ],
  },
  {
    version: "3.111.8-alpha",
    date: "2026-07-30",
    highlights: [
      "Under-the-hood tidying: eighteen of the outside libraries PM is built from moved up to their latest versions, including the one that reads text out of photos. If you have photo text recognition installed, PM will fetch its updated components the next time it reads a picture — nothing to do, and nothing is lost while it does.",
      "One update was deliberately left behind: a testing library that now requires a newer version of Node than PM's build machines run. That is a change worth making on its own rather than smuggled in with routine housekeeping.",
    ],
  },
  {
    version: "3.111.7-alpha",
    date: "2026-07-30",
    highlights: [
      "Under-the-hood tidying: a handful of PM's own safety checks turned out not to be running. The rule that every outside tool PM is built with is locked to an exact, reviewed version was written down in two places and enforced in none. The set of checks that run before a change is saved had quietly shrunk to nine of thirteen — and the four that had gone missing were the ones whose whole job is to notice that sort of drift.",
      "Changes built on top of other changes used to get no checks at all — no tests, no scans, no version check — which is exactly how PM's bigger features are put together. They now get the same treatment as everything else.",
      "PM also builds its actual interface as part of every check now, and proves its Linux packages still assemble, rather than finding out about either on release day.",
    ],
  },
  {
    version: "3.111.6-alpha",
    date: "2026-07-30",
    highlights: [
      "Under-the-hood tidying: two of the checks that keep PM safe were themselves unchecked. The part that strips anything dangerous out of a document before showing it to you is now tested against real hostile input, and the isolation PM runs untrusted files inside now has tests that fail loudly instead of quietly reporting success without having looked.",
      "The test runner also collected only some of the folders it was meant to, so a new test could have been written, committed, and never run once. It now covers everything, and a check makes sure nothing slips outside it again.",
    ],
  },
  {
    version: "3.111.5-alpha",
    date: "2026-07-30",
    highlights: [
      "Deleting a photo or a spreadsheet now actually deletes it. PM was treating both as if they were files indexed from a cloud account — removing them from search but leaving what it had written in your vault behind, so the next time you rebuilt the index they came back. If you kept a copy of a photo in your vault, that picture is now removed with it too; before, it stayed there with nothing left to say where it was.",
      "The confirmation window tells you the truth about which of those is happening. It used to say a photo you dragged in off your desktop was safe in a cloud account you had never connected. Now it says plainly that PM's copy in your vault is going, and that the file you imported from is left alone.",
      "Files indexed from Google Drive, OneDrive or a watched folder are unchanged and always were: PM only ever removes its own index entry, and never touches the file where it lives.",
    ],
  },
  {
    version: "3.111.4-alpha",
    date: "2026-07-30",
    highlights: [
      "Text recognition in photos works again. PM runs the part of it that reads your files in a locked-down process with no internet access — which is right, except that the text reader downloads its own models the first time it runs, so it could never get them. Every photo came back as if it held no text at all: a receipt, a whiteboard, a screenshot of a message, all indexed as blank and unsearchable. PM now fetches those models the same way it fetches the search and speech models, before the reading starts.",
      "They also live alongside everything else PM downloads now, rather than inside its Python folder, so reinstalling that folder no longer throws them away and re-downloads them.",
      "And when text recognition genuinely can't run, the photo now says so on its row while it's being added, instead of quietly landing as a photo with no text in it.",
      "PM is harder to trip up with a booby-trapped file. A small document can be built to unpack into something enormous — PM checked spreadsheets for that, but not Word, PowerPoint or e-book files, which take the same shape. It now checks all of them. Very wide spreadsheets are handled properly too: PM limits how much of a sheet it takes in, and says in the sheet's summary when it left something out, rather than choking on it.",
      "If PM was closed at the wrong moment while reading a file, it could leave a plain, unencrypted copy of that document in its own working folder. It clears any it finds the next time it starts.",
      "Under the hood, PM's document engine can no longer be knocked over by a chatty library printing where it shouldn't.",
    ],
  },
  {
    version: "3.111.3-alpha",
    date: "2026-07-30",
    highlights: [
      "Removing your data now stops PM writing anything else down, everywhere at once. Clearing your interface preferences only sticks if nothing puts them back — and two things could. The floating briefing window keeps its own copy of your theme, and it can't be told to stop, so PM now closes it before erasing rather than leaving it running. And the main window re-saved every theme setting whenever your system switched between light and dark or you clicked away, which was enough to restore the lot seconds after they'd been cleared.",
      "The background syncs also stop as soon as you start removing data, instead of carrying on against a machine that's being erased while the final screen is open.",
    ],
  },
  {
    version: "3.111.2-alpha",
    date: "2026-07-30",
    highlights: [
      "On Mac and Linux, the folders PM had just erased could reappear while you were still reading the message saying they were gone. There's no uninstaller to hand over to on those systems, so PM stays open on the final screen — and anything still ticking over in the background would ask PM where its folder was, which quietly re-created it. It no longer does: once you've erased everything, PM stops making that folder for the rest of the session.",
      "Removing everything now also clears the interface preferences behind the app window, even if you didn't tick that box. PM was deleting the folder those live in whenever you removed everything else, but only emptying the window's own copy when the preferences box was ticked — so in that one combination the system could write them straight back over the folder that had just been deleted.",
      "And a sync running in the background can no longer rebuild PM's Python components into the folder you just erased — a few hundred megabytes that would have reappeared minutes later, on a machine you thought was clean.",
      "PM also tidies up once more as it closes, in case anything was written back while the final screen was open.",
      "Windows was never affected by any of this: it hands straight over to the uninstaller and closes immediately.",
    ],
  },
  {
    version: "3.111.1-alpha",
    date: "2026-07-30",
    highlights: [
      "PM no longer leaves hundreds of megabytes in a cache folder you'd never think to look in. Setting up its Python components left every downloaded package sitting in a system-wide cache outside PM entirely, where removing PM never touched it. That cache is no longer written at all — the same downloads happen, nothing is kept afterwards.",
      "The same for the model downloader's own scratch cache, which quietly wrote to your home folder instead of PM's. Everything PM downloads — the search model, the speech model, and their caches — now lives in one folder under PM, so it goes when PM goes.",
      "Old installers left over from updating PM are cleared out too. Each update downloaded an installer to your temp folder and never removed it, so they built up around 100 MB at a time.",
      "The removal summary now points you at the things PM caused but shouldn't delete for you — models you pulled through the Local AI tab belong to Ollama, and on a Mac, the microphone permission is macOS's to forget. It tells you where they are and how to clear them, rather than leaving you to find them.",
      'And it stops saying "everything PM stored is gone" when it has just listed things it left behind.',
    ],
  },
  {
    version: "3.111.0-alpha",
    date: "2026-07-30",
    highlights: [
      "An interrupted sync no longer leaves a copy of your document sitting in your temp folder. To read a file from Google Drive or OneDrive, PM saves it briefly as an ordinary unencrypted file so the converter can open it, then deletes it — but if PM was closed or crashed at the wrong moment, that copy stayed there forever, outside your vault and outside everything the erase knew about. PM now clears them at startup and when you remove your data.",
      "\"Remove PM data\" now removes a vault you moved. If you'd moved your vault to another folder, the erase deleted the key that opens it and left the folder behind — so for a private vault, whose notes aren't encrypted, your notes stayed readable on disk while PM could never open them again. It now deletes PM's files there too, and only ever PM's: a folder that also holds your own files keeps them, and keeps the folder.",
      "It won't touch a vault that isn't yours. A shared vault another account on the machine created is left exactly where it is, and PM tells you where that is rather than quietly skipping it.",
      "Deleting a shared vault is now the owner's to do. Anyone who had joined a shared vault could delete it for everybody, with no warning that it wasn't theirs. That option is now hidden for a vault someone else set up, and when PM can't tell who set it up, it says so before you go ahead.",
      "The summary now lists anything PM left behind, with the exact path. Previously a locked folder, or a vault belonging to another account, simply went unmentioned while the screen said everything was gone. If PM can't remove something, it now shows you where it is so you can finish yourself.",
      "The removal screen asks you to back up first, before anything is ticked — PM keeps no copy of what it erases, and can't put any of it back.",
      "On Windows, removing PM completely now also clears the sandbox profile PM created for its document reader, which used to be left behind in your app-data folder for good.",
    ],
  },
  {
    version: "3.110.2-alpha",
    date: "2026-07-30",
    highlights: [
      "Your notes can no longer be switched back to plain text by editing a file next to them. Every vault keeps a small settings file recording whether notes are encrypted at rest, and PM checked it for tampering — but if the check failed it re-signed whatever it found, so the change became the new truth and the warning went away after one launch. A shared vault could be quietly turned back to plain text by someone who could reach the folder but didn't know the passphrase. PM now refuses to accept a change that weakens protection, keeps encrypting regardless of what the file claims, leaves the altered file alone rather than signing it, and keeps telling you until it's put right.",
      "You'll now be told about that at startup too. The warning only ever appeared when you typed your passphrase, so a vault that opened on a remembered key reported the problem to a log file nobody reads. It now shows on any launch, and the Vault panel reports whether your notes are actually being encrypted rather than what the settings file says they are.",
      "A crash while PM is saving no longer costs you the whole note. Notes, chat transcripts and saved photos were written straight over the top of the old file, so an interruption part-way left a half-written file — and on an encrypted vault half a file is unreadable, not partly readable. PM now writes to a temporary file, flushes it to the disk, and swaps it into place in one step, so you always end up with either the old version or the new one. Chat transcripts benefit most: the whole transcript is rewritten on every reply.",
      'Restoring a backup is much more careful about what it replaces. PM offered to move a restored vault into its usual home when that home looked empty — but "empty" only counted imported documents, so a vault holding your projects, milestones, flags, chats, connected calendars and preferences could be treated as blank and erased. It now checks for anything you put there yourself.',
      "A restore also stops before it fills your drive, with a clear message instead of a raw disk error, and no longer leaves a decrypted copy behind if the last step fails.",
      '"Remove PM data" no longer leaves stray files behind, and tells the truth about what it left. It missed the list of accounts linked to a shared vault, and it described a vault you had simply moved to another folder as "the shared vault folder" — now it names the actual folder and says plainly that it hasn\'t touched anything inside it.',
      "On Linux, \"Remove PM data\" now really removes everything. The folder holding PM's browser-side data — your interface preferences, and the cookies and stored data behind the app window — was skipped on Linux, and unlike Windows there's no uninstaller afterwards to catch it, so it stayed on the machine for good. It's now erased along with the rest. On Mac, the cookie file sitting beside that folder is cleared too.",
    ],
  },
  {
    version: "3.110.1-alpha",
    date: "2026-07-30",
    highlights: [
      "Approving a file in Review no longer leaves it attached to the inbox it came from. Filing a document recorded the project it was moving out of as an extra project it still belonged to — written into the file itself, so it survived a re-index — which meant the Unsorted inbox slowly accumulated a link to every document you had ever approved. Nothing about this was visible on screen, so the only sign was the inbox looking oddly full in project pickers.",
      "Renaming or merging a project now really retires the old name. The same underlying mistake meant every document the rename touched kept a link to the name you had just replaced, so it kept reappearing in pickers and tag suggestions and came back after a re-index. Both are fixed at the one place that derives which projects a document belongs to, so any future filing screen gets it right for free.",
      "Deleting a project now finds chats you filed into it by hand. A chat you started generally and later filed into a project was reachable by neither half of the delete, so deleting that project either failed with an unhelpful error or reported success while the project quietly came back. It is now found the same way whether it was started in the project or filed into it afterwards.",
      "If one of these bulk changes fails part-way, PM now puts every file it had already rewritten back the way it was, instead of leaving the files and the database disagreeing about where things are filed.",
    ],
  },
  {
    version: "3.110.0-alpha",
    date: "2026-07-29",
    highlights: [
      "Filing suggestions now arrive with the files, a few at a time, instead of all at once at the end. Start a sync with AI suggestions on and each file turns up in Review already carrying a proposed project, importance and tags — so you can approve the first handful while the rest are still being indexed, rather than waiting for the whole sync to finish before anything is suggested.",
      "Every file gets suggestions now, however it arrived. Suggestions used to be triggered only when a sync finished, so a file picked up by the folder watcher, or one you dragged in by hand, sat without any until the next sync completed or you opened Review. Nothing about how a file reached PM should change whether PM offers to file it.",
      "None of this spends anything you haven't opted into: it's the same AI-suggestions switch, re-read as each file lands, and PM never asks twice about a file it has already suggested for.",
    ],
  },
  {
    version: "3.109.3-alpha",
    date: "2026-07-29",
    highlights: [
      "A sync you queue now runs for the account you asked for, and its row says so. Queuing a second account mid-sync only recorded that something had been asked for, not what — so PM answered by re-syncing every account, and the row you queued sat on “Queued” the whole time instead of taking its turn. Each queued account now gets its own pass, in the order you asked, and its row switches to “Syncing…” when it comes up.",
      "Pressing “Sync now” during an automatic background sync no longer quietly skips “Shared with me”. The background check leaves that part out on purpose — it has no quick way to spot changes and is too slow to repeat every few minutes — and a request folded into one inherited that, so the sync you asked for silently covered less than a sync you started yourself. A queued sync now always includes it.",
      "Stopping a sync now also drops anything queued behind it, rather than starting the next one straight after.",
    ],
  },
  {
    version: "3.109.2-alpha",
    date: "2026-07-29",
    highlights: [
      "The briefing panel gets the resize cursor too. It draws its own frame like the main window, so on Linux its edges could be dragged but never showed it — the fix in the last update only reached the main window, because the panel deliberately doesn’t share its chrome.",
    ],
  },
  {
    version: "3.109.1-alpha",
    date: "2026-07-29",
    highlights: [
      "Approving a single file in Review now works while suggestions are still being written. The row’s Approve button stayed clickable once that file had its suggestion, but pressing it did nothing at all — so the one thing you’d naturally do while watching files arrive was the one thing that silently failed.",
      "Re-propose no longer leaves the old suggestions on screen. It cleared everything behind the scenes but left the previous run’s project, tags and importance showing in each row — so approving one filed values from a run you’d just discarded, and recorded them as a correction you never made.",
      "Suggestions no longer go missing when two syncs finish together. A second batch of files arriving while suggestions were being written was quietly dropped instead of being picked up next, so those files sat unsuggested until something reloaded the Review tab. They now queue.",
    ],
  },
  {
    version: "3.109.0-alpha",
    date: "2026-07-29",
    highlights: [
      "Files now appear as they’re indexed, instead of all at once when the sync ends. Start a Google Drive, OneDrive or folder sync — or just drag files in — and each one shows up in Documents and in Review the moment it’s stored, with the Review badge counting up beside it. A long sync used to be a progress bar and nothing else until it finished; now you can watch it fill, and start approving the first files while the rest are still arriving.",
      "It works whichever screen you’re on. Files landing while you’re in Chat are waiting for you when you open Review — PM keeps track of them rather than only noticing what arrived while you were looking.",
    ],
  },
  {
    version: "3.108.5-alpha",
    date: "2026-07-29",
    highlights: [
      "Queuing a sync while another one is running no longer looks like it was ignored. Asking to sync a second account mid-sync folds it into a follow-up pass — but PM announced “finished” at the end of every pass, so the progress bar and the Queued badge both cleared while the queued work was still going. It looked like your request had been dropped when it was actually running. PM now says a sync is finished once, when the whole run is genuinely over.",
      "The summary at the end of a sync now counts the whole run. Because each pass reported separately, a sync that indexed fifty files could finish by telling you it indexed none — the follow-up pass found nothing new and had the last word. The totals now add up across every pass.",
      "Filing suggestions no longer run twice per sync, or at all after you press Stop. They were triggered by that same per-pass “finished”, so a queued sync generated suggestions once over a half-built index and again at the end. Stopping a sync now also stops the suggestions that would have followed it — pressing Stop used to trigger the largest batch of model calls PM had.",
    ],
  },
  {
    version: "3.108.4-alpha",
    date: "2026-07-29",
    highlights: [
      "On Linux, the window edges now show a resize cursor. PM draws its own window frame, and on Linux the toolkit underneath performs the resize but never changes the pointer — so dragging an edge worked, and there was no way to tell where the edges were except by guessing. The pointer now becomes a resize arrow within a few pixels of any edge or corner, matching exactly where the drag will actually take. Windows and macOS were never affected; they get the cursor from the operating system. Note the arrow deliberately stays hidden while the window is maximised, because resizing genuinely isn’t available then.",
    ],
  },
  {
    version: "3.108.3-alpha",
    date: "2026-07-29",
    highlights: [
      "“Delete oldest, keep 5” now trims what it can and tells you about the rest. Google Drive only lets PM touch files its own sign-in created, so backups uploaded before you last reconnected your Google account can be listed but not deleted. PM used to stop at the first one it wasn’t allowed to remove, so nothing at all got trimmed and you got a wall of Google error text. It now moves every archive it can, then says plainly how many it had to leave and that you can delete those in Drive yourself. The banner also refreshes now even when the trim fails, instead of sitting there showing the old count.",
      "Disconnecting Google Drive no longer quietly breaks your backups. The Drive connector and Drive backups share one sign-in, and disconnecting the connector was handing that sign-in back to Google — which is what put PM in the position above. If the account is also your backup destination, PM now keeps the sign-in and only stops using it to index files.",
      "Automatic backups no longer trim in silence. A scheduled backup that couldn’t tidy up old archives said nothing at all; it now shows up in the same banner that already reports a destination problem.",
    ],
  },
  {
    version: "3.108.2-alpha",
    date: "2026-07-29",
    highlights: [
      "A folder PM can’t watch no longer turns into a sync that never stops. Watching a folder for changes costs the operating system one watch per sub-folder, out of a budget shared with everything else running on your machine — so it can genuinely run out. PM used to note the failure and try again five seconds later, and because starting to watch a folder also kicks off a catch-up sync, it would sync, fail, and sync again indefinitely: a progress bar that restarted every few seconds and a Stop button that didn’t take. PM now waits five minutes between attempts, says once what happened and that the limit is usually the cause, and hands back the watches it had already taken instead of stranding them. Your folder still stays up to date — the regular sync covers it either way.",
      "PM also stops following shortcuts when watching a folder. It never indexed anything behind a symlink, but it was still spending watches on those folders — and a shortcut pointing back at its own parent could spend a great many.",
    ],
  },
  {
    version: "3.108.1-alpha",
    date: "2026-07-29",
    highlights: [
      "Settings → Storage no longer implies a library is missing when it’s already there. Shared libraries like OpenCV, shapely and scipy sit under the feature that uses them, and while that feature is installed they can’t be removed on their own. They used to be labelled “Needs a step first”, which read like something still to download — so photo text recognition looked half-installed when it was working fine. They now read “Installed — in use”, with the same pill pointing at what to remove first. The help text for the tab also covers the photo text recognition libraries, which it had never mentioned.",
    ],
  },
  {
    version: "3.108.0-alpha",
    date: "2026-07-28",
    highlights: [
      "On a Mac, “Remove PM data” now actually removes everything. macOS keeps a surprising amount on an app’s behalf outside its own folder — the window’s stored preferences, cached web data, cookies and saved window state — and PM had never cleared any of it. So a Mac reinstall quietly remembered things a fresh install shouldn’t, like whether you’d turned on developer mode. Tick “App preferences” and those are gone too.",
      "PM also stops giving Mac users Windows instructions. It used to finish by telling you to uninstall from “Windows Settings → Apps”, which doesn’t exist on a Mac — and there’s no uninstaller there either. It now says what’s actually true: your data is gone, and the last step is to drag PM from Applications to the Trash. There’s a button to open Finder with it selected. PM won’t delete itself — that stays your call.",
    ],
  },
  {
    version: "3.107.1-alpha",
    date: "2026-07-28",
    highlights: [
      "PM now stops instead of guessing when it can’t read your saved keys. If your login keychain is locked, or the entry PM keeps its secrets in is damaged, PM used to see “no keys saved” and helpfully create a new database key — writing over the only key to your vault. It now tells the difference between “nothing saved yet” and “can’t read what’s saved”, leaves your vault exactly as it is, and offers Retry. On a Mac it also no longer asks for your keychain password again and again if you dismiss the prompt once.",
    ],
  },
  {
    version: "3.107.0-alpha",
    date: "2026-07-28",
    highlights: [
      "You can change which project a document really belongs to. Click any linked project to make it the primary one, or remove the primary and the next in line takes over. The primary pill had no controls at all before, so a document filed somewhere you didn’t intend was something you could see and not correct.",
      "Teach → Tags is back on the normal display setting. Renaming a tag, folding near-duplicates and the whole re-tag pass were only drawn on the Power setting — which isn’t the default — so the repair the last two updates asked you to run sat on a screen most people never see.",
      "The message box says when it will queue. While PM is answering it reads “Queue a message…” and the button says Queue, so pressing Enter mid-answer looks like what it is rather than like being ignored.",
      "Turning the duplicate check on now works from wherever you are. Switching it on in Settings while looking at Documents left that tab unchanged until you navigated away and came back; the “Check for duplicates” action now appears as soon as Settings closes. The one-time suggestion also stays gone once you’ve acted on it, instead of returning if you ever switch the check off.",
      "If a message fails to send and you delete it, PM stops offering to try that message again. It says the rest are still waiting and offers Continue queue — the same two choices as before, worded for what is actually left.",
      "Searching for a tag the way PM writes it now finds something. @“Atlas, Inc.” — the form PM’s own @ menu inserts for a name with a space in it — matched nothing in search, and only the first @ in a search was understood.",
    ],
  },
  {
    version: "3.106.3-alpha",
    date: "2026-07-28",
    highlights: [
      "A rule PM enforces on itself now describes itself accurately. PM requires every outside package used by its build scripts to be named, justified, and pinned to one exact version. The written rule still said a single package had ever been allowed through — there are two — and it never mentioned that the second one carries an exemption from the exact-version part, or that an exemption has to state its own reason before the check will accept it. All of that is now written down. The automated check was right the whole time; only the description of it had fallen behind.",
      "Nothing about PM behaves differently. It matters because a rule whose own wording can't be trusted is a rule that quietly stops being followed.",
    ],
  },
  {
    version: "3.106.2-alpha",
    date: "2026-07-28",
    highlights: [
      "The Local AI settings screen is now covered by tests. It is the biggest screen in PM and had none, which is how two recent problems reached you unnoticed: a saved endpoint token you could not remove, and embedding models offered as if you could chat with them. The new tests cover the places where the screen could tell you something untrue — that a token is gone when it is not, or that a model is usable when it is not.",
      "Nothing about PM behaves differently here. This is a net under work that already shipped.",
    ],
  },
  {
    version: "3.106.1-alpha",
    date: "2026-07-28",
    highlights: [
      "The script that builds PM's model list is now tested. It decides the real size of every model PM recommends, and the fit calculator treats those numbers as measured fact — so an error there does not fail loudly, it quietly mis-sizes models against your machine. Nothing checked any of it until now, because the script needs the network and so never ran in CI. Its rules are now unit-tested without touching the network at all: 24 tests covering how it matches a quantization exactly (so “Q6_K” never also claims “Q6_K_L” and inflates a size), how it adds up a split model, how it picks a single vision projector rather than summing spare copies, how it works out a mixture-of-experts model's active size from the file header, and which models it refuses to guess about at all.",
      "Nothing about PM behaves differently. This is coverage for machinery that already worked.",
    ],
  },
  {
    version: "3.106.0-alpha",
    date: "2026-07-28",
    highlights: [
      "Local AI stops offering you models that can't hold a conversation. Ollama and LM Studio serve embedding models — the kind that turn text into numbers for search — from the same address as your chat models, and PM was listing them as though you could assign one. They're still listed, greyed out with the reason, rather than quietly removed: a model you can see in Ollama but not in PM looks like PM is broken. Setting PM up for the first time without an API key no longer risks binding one of them to everything by accident.",
      "A downloaded vision model is sized correctly again. PM was counting the vision part of the model twice, so a multimodal model you already had could be reported as needing about a gigabyte more memory than it does — enough to be told to halve your context, or to stay on the cloud, when neither was necessary. It was also adding up every spare copy of that vision file sitting in the same folder instead of the one that actually loads.",
      "You can remove a saved endpoint token without disconnecting. Until now the only way to get rid of one was Disconnect, which also wiped the address and both of your model assignments. There's a Forget token button beside it, and a saved token can no longer be left stranded in your keychain if the address it belonged to was rejected.",
    ],
  },
  {
    version: "3.105.2-alpha",
    date: "2026-07-28",
    highlights: [
      "PM's build scripts now have to justify every outside package they use. They were already meant to stay dependency-free, but that was a habit written in a comment, and nothing checked it. Now a check does: an outside package needs an entry saying which script uses it and why, it has to be development-only so it can never reach your installed copy, and it has to be pinned to one exact version rather than a range that can move underneath us.",
      "This one is entirely under the bonnet — nothing about PM looks or behaves differently. It matters because the fewer outside packages sit near the machinery that builds and checks PM, the fewer ways something unwanted can arrive in a release.",
    ],
  },
  {
    version: "3.105.1-alpha",
    date: "2026-07-28",
    highlights: [
      "Corrected something PM claimed about itself. The 3.85.2 summary said PM reads a dedicated Intel Arc card's real memory on Linux, as though that had been tried. It hasn't — that support is written from the Linux graphics driver's own documentation and has never run on such a machine, which the detailed entry for it said plainly at the time. The summary now claims only Windows, where it has actually been exercised. The Linux support is unchanged and still there; only the claim about it was too confident.",
      "Notes inside PM's own source no longer claim checks that haven't happened. Two parts of the local-AI code described themselves as verified against a real local model server. That verification is still outstanding, and they now say so instead.",
    ],
  },
  {
    version: "3.105.0-alpha",
    date: "2026-07-28",
    highlights: [
      "Queued messages can be edited before they go. Click one to change it, Enter to keep the change, Escape to leave it as it was — handy when PM’s answer half-covers what you were about to ask.",
      "While you’re editing, nothing sends. PM waits until you’re done rather than firing off the version you were part-way through fixing.",
    ],
  },
  {
    version: "3.104.1-alpha",
    date: "2026-07-28",
    highlights: [
      "Fixed the whole window sliding up out of view after sending a message. Snapping to the newest reply was scrolling the entire app rather than just the conversation, and nothing scrolled it back.",
    ],
  },
  {
    version: "3.104.0-alpha",
    date: "2026-07-28",
    highlights: [
      "You can keep typing while PM is still answering. Press Enter and your message waits its turn instead of being blocked — up to three can queue up, and they show above the box so you can see what’s coming.",
      "They go one at a time, in order, each waiting for the previous answer to finish. Changed your mind because the answer covered it? Take one back with the ×.",
      "If a message fails to send, PM stops there rather than firing the rest on top of it. Nothing is lost or reordered, and there’s a Try again.",
      "Queued messages aren’t kept if you switch chats or close PM — they were written for the conversation you were in.",
    ],
  },
  {
    version: "3.103.0-alpha",
    date: "2026-07-28",
    highlights: [
      "PM can now look through your library for documents you have twice. Turn it on under Settings \u2192 Data & Security, and a \u201cCheck for duplicates\u201d action appears above your Documents list.",
      "It looks two ways. It compares the opening of each document with capitals, punctuation and spacing ignored \u2014 so the same file that reached PM by two different routes still matches \u2014 and it compares what each document is about, which catches the same report saved as both a Word file and a PDF.",
      "Every pair tells you why it was flagged, shows you both documents to open and read, and removes nothing on its own. When you do remove one, PM names the exact document and leaves the other alone; for something in a connected account it drops only its own pointer and never touches your file there.",
      "It is off until you ask for it, because some pairs won\u2019t be duplicates at all \u2014 anything built from a template shares an opening, and a run of invoices reads very alike.",
    ],
  },
  {
    version: "3.102.0-alpha",
    date: "2026-07-28",
    highlights: [
      "Your conversations now live in their own folder. Open your vault and you\u2019ll find a chats folder beside your documents instead of hundreds of chat files mixed in among them.",
      "Existing chats move themselves the next time PM opens \u2014 a plain rename, nothing re-encrypted, nothing re-indexed. If PM is interrupted halfway it picks up where it left off, and if it ever finds two files claiming the same name it leaves both alone and says so rather than guessing.",
      "There is no project in the path, deliberately: a chat can belong to several projects now, so the folder keeps your chats together and PM keeps track of which projects they belong to.",
      "Under the hood: rebuilding, changing your passphrase, and exporting to plaintext Markdown all reach the new folder. Turning encryption on used to leave a chat\u2019s next message in a second file \u2014 that\u2019s fixed too.",
    ],
  },
  {
    version: "3.101.0-alpha",
    date: "2026-07-27",
    highlights: [
      "Your tags are now yours to edit. Teach \u2192 Tags lists every label with how many documents carry it: click one to rename it everywhere, or \u00d7 to take it off every document. Both go through your vault, so they stick.",
      "PM also points out labels that look like the same thing \u2014 tax and taxes, chair and chairs \u2014 and folds one into the other in a click. It keeps whichever spelling more of your documents already use.",
      "The re-tag pass now asks you first. It suggests a set of tags for your library and stops there; you drop the ones you don\u2019t want and add the ones you know you need, and only then does it label anything. The vocabulary is the one decision the whole pass turns on, and it\u2019s forty words \u2014 far quicker to check than the result of getting it wrong.",
      "Removing a tag asks first and tells you how many documents it comes off. Nothing else about them changes and no files are deleted.",
    ],
  },
  {
    version: "3.100.0-alpha",
    date: "2026-07-27",
    highlights: [
      "You can re-tag your whole library in one go. Teach \u2192 Tags reads everything you have, picks one set of tags that actually suits it, and re-labels every document from that set.",
      "This fixes something that was quietly broken. Tags were proposed a few documents at a time, so PM kept coining labels for whatever was in front of it \u2014 'ammun', 'chair-application', 'placement'. Each is a fair description of one file and useless as a tag, because a tag that lands on one document groups nothing.",
      "The difference is that the vocabulary is chosen FIRST, with your whole library in view, and then everything is labelled from that one set. Choosing tags a few documents at a time cannot do better than the few documents it saw \u2014 which is how you ended up here.",
      "Nothing is written until you say so. You get a before-and-after per document, you can untick any you disagree with, and PM only touches tags \u2014 your projects and importance levels are left exactly as they are.",
      "It tells you what it will cost before it starts, because it does cost: it re-reads your library through the model, roughly one call per twelve documents.",
      "New vaults get this for free. Until now the very first sorting run had no tags to work from, so it coined labels a few documents at a time \u2014 the same mess, on day one. It now picks a vocabulary for everything waiting first, and files against that, so you only need the Teach \u2192 Tags pass if you already have a library to repair.",
      "How many tags PM picks now depends on how much you have \u2014 about one per five documents, so a small library isn\u2019t crushed into a handful of vague buckets and a big one isn\u2019t forced through forty.",
    ],
  },
  {
    version: "3.99.0-alpha",
    date: "2026-07-27",
    highlights: [
      "You can now point a chat at a tag. Type @ in the message box and PM offers the projects and tags you have; pick one and that message searches it.",
      "Pinning never shuts anything out — it adds and it leans. In a project chat it REACHES FURTHER: the project's own files stay, and the tagged ones join them, so you can ask the Marketing chat about something filed under Coding without leaving. In the main chat, which already searches everything, it LEANS: the tagged files are ranked up, and the rest of your library is still there. That is the difference between the two chats, and pinning a tag doesn't blur it.",
      "It lasts exactly one message. Nothing is remembered, nothing is turned on: a chat scoped to a project still answers only from that project the moment you stop asking it not to.",
      "Leaning is not forcing. A tag you pin only lifts files the question actually matched — asking about a tax deadline while pinning @marketing will not drag in marketing files that say nothing about tax.",
      "The tag itself is taken out of the question before the search runs, so what PM matches on is what you actually asked, not the name of the tag you pinned.",
      'The tags you pinned are highlighted in your sent message, so you can see which ones PM recognised and which were just words. A name with a space in it goes in quotes: @"Atlas, Inc.".',
      "Your free-form tags now do something. They were labels PM stored and never used; they can now steer a chat, and searching (Ctrl-K) finds a file by any of its tags or projects, not just its name.",
      "And they should group better from here: when PM files new documents it is now shown the tags you already have and asked to reuse them, instead of coining a near-duplicate. A label that only ever lands on one file is not doing anything.",
      "In a project's file list, files no longer repeat the name of the project you are already looking at — you just see the OTHER projects a file belongs to, which is the part you cannot infer.",
    ],
  },
  {
    version: "3.98.0-alpha",
    date: "2026-07-27",
    highlights: [
      "A document can now belong to more than one project. The Project field is a list you build like the To: line of an email — type a name to add it, and a name PM hasn't seen creates a new project. The first one is the primary project, and the rest are links.",
      "The primary project is the one that owns the document: it's where the document counts as activity, and where it sits on the Map. Everywhere a document appears you can see which project is its primary and which are links, so if it's filed somewhere you didn't expect, you can see that and change it on the spot.",
      "A project's chat can now answer from documents linked into it, not just the ones filed there — which is the point of linking one in. A project's file list shows them too, marked Linked with the project they're really filed under.",
      'Project names now behave like names. "Atlas, Inc." stays exactly that instead of being torn in two at the comma, and a project matches however you capitalise it while keeping the spelling you chose.',
      "Existing vaults carry over untouched: every document keeps the project it was already in, and nothing needs re-filing. Merging, renaming or deleting a project moves or removes its links along with everything else.",
    ],
  },
  {
    version: "3.97.0-alpha",
    date: "2026-07-27",
    highlights: [
      "You can now delete a single document. A Delete option appears on each row in the Documents tab and in a project's file list, so you can remove something from wherever you happened to find it. It always asks first, and it tells you exactly what will go — because that differs: deleting a file removes it from your vault and from search; deleting a saved chat removes the conversation and its messages too, not just the searchable transcript; and deleting something indexed from Google Drive or OneDrive only removes PM's copy of the index, never the original in your cloud account.",
      "The Delete option stays out of the way until you hover a row, so it can't be hit by accident, but it's still reachable by keyboard.",
    ],
  },
  {
    version: "3.96.0-alpha",
    date: "2026-07-27",
    highlights: [
      "You can now delete a project, and decide what happens to everything in it. Open it in Focus and choose Delete. You pick separately what becomes of its files (move them to Unsorted, or delete them), its chats (keep them as ordinary chats, or delete them), and its name — either anything mentioning that name in future files to your inbox, or the name is freed so it can start a fresh project later. PM counts up exactly what the project holds before you choose, and asks you to type its name to confirm, because none of this can be undone from inside PM.",
      "Its milestones are always deleted, and PM says so rather than offering a choice — there's nowhere sensible to move a dated milestone whose project no longer exists. Its deadlines, priority and the record of when you worked on it go with it. Reminders hanging off those milestones are cleaned up properly instead of being left behind with nothing to point at, and anything you'd told PM about the project in Teach is kept rather than quietly destroyed.",
      "Files indexed from Google Drive or OneDrive are only ever unlinked from PM. Choosing to delete them removes PM's copy of the index and nothing else — the original files in your cloud account are never touched.",
      "Clicking a source in an old answer that has since been deleted now tells you so. It used to do nothing at all, which looked like a broken link; it now says the file has been deleted and that re-ingesting it will bring it back. Past answers still list it because PM records an answer's sources at the moment it writes the answer.",
      "A project card on the pinboard survives its project being deleted. The card stays where you put it and simply loses its link, rather than vanishing from your board.",
    ],
  },
  {
    version: "3.95.0-alpha",
    date: "2026-07-27",
    highlights: [
      "You can now merge one project into another. If a project turns out to have always been part of a bigger one — a landing page redesign that was really just Marketing — open it in Focus, choose “Merge into…”, and everything it holds moves across: its files, its chats, its milestones, and its history of when you worked on it. The old project is then deleted. Because that can’t be undone, PM counts up exactly what will move and tells you before you commit — “12 files, 5 chats and 3 milestones will move to Marketing” — and asks you to type the name of the project you’re keeping. PM also remembers the old name, so if it turns up in a document later it files it under the project you kept rather than quietly recreating the one you just merged away.",
      "“Part of” has been removed. It looked like a way to group projects together, but it did close to the opposite: naming a parent hid the project’s own status and showed “Part of X” in its place, so a project with a deadline next week could sit there looking like a footnote. Grouping projects properly is coming separately; the one thing “Part of” was genuinely useful for — a project that never deserved to be its own project — is what the new merge does, and it does it honestly by actually moving everything and deleting the source instead of leaving a half-project behind. If you had set a parent on anything, nothing is lost: the project simply shows its real status now.",
      "Projects with others waiting on them still stand out. PM works out a priority on its own for any project you haven’t set one for, by looking at how many other projects depend on it. That used to count both “is blocked by this” and “is part of this”; now it counts only what’s genuinely waiting on it, which is a sharper signal and, in practice, the one people actually set.",
    ],
  },
  {
    version: "3.94.0-alpha",
    date: "2026-07-27",
    highlights: [
      "Milestones now have one control instead of two. The tick-box is gone — set a milestone to Done in its progress dropdown and it crosses out exactly as before. Having both meant two things to keep in step and two places to look to learn one answer, and they could drift apart. Everything the tick-box did still happens, including bringing a deadline reminder back when you move a milestone off Done.",
      "Project timelines on the pinboard now show progress too. In list view each milestone has the same progress dropdown as the project page; on the track view the dot on the line fills up as a milestone moves from Not started through to Done, so you can read the state of a project at a glance without opening it.",
      "Rating an answer in chat now visibly does something. The thumbs up and down have always been recorded — but the only sign you had chosen one was a colour change that could never appear, because the buttons were emoji and emoji ignore colour. They are now drawn icons, and the one you pick fills in.",
      "Apple and subscribed calendars can be marked work or personal, like the rest. The setting arrived last version for Google and Outlook calendars only, because subscriptions are managed in a different part of Settings and were missed.",
      "The date picker opens when you click a date, and no longer disappears. Clicking the date itself opens the calendar — the little 📅 button next to it is gone. On pinboard timelines the picker was being cut off by the edge of the card and could not be seen at all; it now floats above everything and stays on screen wherever the field happens to sit.",
      "Timeline cards on the pinboard can hide finished milestones. A “Done” tick sits next to “Past”, and each answers its own question — one hides what is behind you by date, the other what you have finished, and a milestone can easily be one without the other. It is remembered separately from the same setting on the project page, so tidying the board doesn’t tidy your project view too.",
      "The left sidebar sits properly under the window bar again. A row that used to hold the app’s name was still reserving its space after the name moved into the title bar, leaving an odd gap above Search on every tab. “+ New” has moved out of that row onto the Chats row, so it no longer pushes the whole sidebar down to claim a line of its own, and it stays put now instead of coming and going as you change tabs.",
      "Starting a new chat now says which kind you’ll get. “+ New” on the Chats row always makes a normal chat. Open a project and a second “+ New” appears above that project’s own conversation list, which is the one that makes a chat inside the project. Before, there was a single button that quietly did one or the other depending on where you happened to be — and if you weren’t in a project, the only other way to start a project chat was to wait for PM to notice your last one had gone stale and offer you one.",
      "Milestones in a project line up properly. The date and progress row was indented to sit clear of the done tick-box that used to lead each milestone; with the tick-box gone the indent was hanging off nothing, which read as a gap in the middle of the card.",
      "The Calendars list in the sidebar stays folded shut if that’s how you left it. It sprang back open every time you visited another tab and returned, because it was rebuilt from scratch on the way back with no memory of your choice. It now remembers, across tabs and across restarts.",
      "The chat count beside each project in the Chats tab keeps up. Starting a chat inside a project didn’t reach the list those counts are drawn from, so the number stayed behind until something unrelated refreshed it. It’s now re-read whenever you open the Chats tab.",
    ],
  },
  {
    version: "3.93.1-alpha",
    date: "2026-07-27",
    highlights: [
      "Under-the-hood tidying: PM now keeps a written record of which parts of your data are genuinely yours — the things that would have to travel with you to a second computer — and which are just working copies it can rebuild. Nothing changes in the app. It matters because PM has quietly grown past “your notes live in the vault”: your preferences, your corrections, your resolved reminders and your project history all live in the database now, and a future version that syncs between machines could have left every one of them behind on the old one without anyone noticing. There is also a new check that stops that list going out of date.",
    ],
  },
  {
    version: "3.93.0-alpha",
    date: "2026-07-27",
    highlights: [
      "Subscribed calendars now show who else is invited. Google and Outlook events have listed their guests since the event pop-up arrived, but calendars you subscribe to by link — including Apple ones — were quietly dropping that, so those events always looked like you were the only person going. They don’t any more.",
      "You can now mark each calendar as work or personal, next to its Quiet setting. Nothing acts on it yet — it’s there so PM can eventually tell a 3pm standup from a 3pm dentist appointment, which today it genuinely cannot. Your choice sticks through a re-sync, and calendars you don’t label stay unlabelled rather than being guessed at.",
    ],
  },
  {
    version: "3.92.0-alpha",
    date: "2026-07-27",
    highlights: [
      "You can now tell PM when an answer was useful — or wasn’t. A quiet pair of thumbs sits under any answer that drew on your files; click one, or click it again to take it back. Nothing happens immediately, and that’s deliberate: PM is building up a record of which of your documents actually answered which of your questions, because that is the one thing it would need to learn to search your files better and it has never been keeping it. Opening a source from an answer counts too, without you doing anything. It all stays on your machine, and deleting a conversation deletes its feedback with it.",
      "Under-the-hood tidying: PM can now load a search model stored on this computer rather than only ones it downloads. Nothing uses that yet — it’s the groundwork for a version of PM’s search that has learned from the feedback above.",
    ],
  },
  {
    version: "3.91.0-alpha",
    date: "2026-07-27",
    highlights: [
      "Under-the-hood tidying: when you correct where PM filed something, it now records which version of its filing brain made the mistake. PM judges how well it’s sorting your files by counting those corrections, and every time its filing gets smarter the older marks stop being a fair comparison — without a note of which version each one belongs to, one improvement quietly drags down the score for years. Nothing changes in how you use it; it just means PM can tell its past self apart from its present one.",
    ],
  },
  {
    version: "3.90.0-alpha",
    date: "2026-07-27",
    highlights: [
      "Milestones can now say how far along they are, not just whether they’re finished. Each one carries Not started, In progress, Almost done or Done, so a pitch you’re halfway through reads as halfway through instead of sitting in the same untouched-looking state as one you haven’t opened yet. The tick-box still works exactly as before for when you just want it off the list — the two stay in step, so ticking a milestone marks it Done and moving it off Done un-ticks it.",
      "Under-the-hood tidying: milestones gained a durable place to record that they came from somewhere outside PM — a tracked spreadsheet row, say — so that when that connection arrives later, re-syncing updates the milestone you already have rather than quietly replacing it with a new one and losing anything attached to it.",
    ],
  },
  {
    version: "3.89.0-alpha",
    date: "2026-07-26",
    highlights: [
      "Your connected accounts keep themselves up to date. Google Drive, OneDrive and your local folders are now checked when you open PM and every 15 minutes after that, the way your calendar already was. Until now nothing refreshed them on its own — a file added to a folder you had indexed stayed invisible until you thought to press Sync, which is not something you think to do about a file you don’t know exists. Sync now is still there for when you don’t want to wait.",
      "“Shared with me” refreshes too, about once an hour. It gets its own slower rhythm for an honest reason: everything else can ask Google “what changed?” and get a near-empty answer, but there is no such question for shared items — the whole list has to be walked and compared. Once an hour keeps it current without doing that every few minutes.",
      "Spreadsheets: the reconnect notice is now findable. If you connected a Google account before PM could read Sheets, it needs one reconnect — and the notice saying so sat on a settings page you had no reason to revisit after the first index. It is now a proper callout, and it puts a small dot on Settings and on the Connectors tab so you can find it at all. Accounts connected since then already ask for this permission up front and never see it.",
    ],
  },
  {
    version: "3.88.0-alpha",
    date: "2026-07-26",
    highlights: [
      "Settings tells you when it saved. The footer used to carry a standing sentence saying changes save as you make them — true, but it said the same thing whether or not anything had happened. It now shows a brief “Saved ✓” each time a setting is written, wherever in Settings you changed it. There is a Save button again too; almost nothing needs it, but it commits anything still waiting and always acknowledges, so pressing it is never a no-op.",
      "Settings won't lose an edit you haven't saved. The backup schedule is the one place that holds a draft rather than saving as you type — committing a “keep the last 1” on the way to typing 15 would be its own kind of accident. Switching tabs or closing Settings with one of those outstanding now asks first, and names exactly what would be discarded instead of showing a vague warning.",
      "Clicking outside Settings closes it. As it always did for What’s New and every other dialog. Anything unsaved still stops and asks.",
      "The Day view can show up to six days. Pick the width next to the range control. Week stays seven, and both grids now start wherever you have scrolled them to rather than snapping to a Monday — so you can look at the six days from yesterday, or a week that begins on Wednesday. Today puts it back, and in Week view it restores the ordinary Monday-to-Sunday shape.",
      "Swipe the calendar sideways. A horizontal trackpad swipe or a tilt wheel moves the day window one day at a time, on both the Calendar tab and the Focus tab’s Upcoming grid — neither of which responded to sideways scrolling at all before, because neither is a scrolling area. The ‹ › arrows page by the whole window instead: seven days in Week, however many days you have chosen in Day.",
      "The window chrome shows your version. The title bar now reads “PM alpha 3.88.0” rather than just “alpha”, so you can tell which build you are on without opening anything. The matching PM/Alpha badge at the top of the left sidebar is gone — it sat a few pixels below the one in the chrome and said less.",
      "The calendar can reopen where you left it. Off by default — it opens on today — with a switch in Settings → General if you would rather it kept your place.",
    ],
  },
  {
    version: "3.87.0-alpha",
    date: "2026-07-26",
    highlights: [
      "Dates are typed, not fought with. Every date box in PM — milestones on a project, on the Focus card, and on a pinboard note or timeline — used to be your web engine's own date control. On Linux that meant a popup that wouldn't close when you clicked away, leaving Escape as the only exit. They're now PM's own field: type the date in plain DD-MM-YYYY, or pick it from a small calendar with Today and Clear. It's forgiving about how you type — 4/8, 14.08.2026 and a pasted 2026-08-14 all work, and leaving the year off means this year — and it finally shows dates in the same format as the rest of the app, rather than whatever your operating system happened to prefer.",
      "Checkboxes in a note can be ticked. A checklist on a pinboard note rendered as real checkboxes but they did nothing — the only way to tick one was to open the note and retype the marker by hand. Now you click the box. Clicking anywhere else in the note still opens it for editing, as before.",
      "Notes keep the line breaks you typed. Writing a line of ordinary text underneath a bullet or a checklist quietly folded it into the item above, so two lines became one run-on line. Notes pasted from another app with Windows line endings were being mangled too. Both are fixed, and your existing notes will simply start rendering the way you wrote them.",
      "Checklists line up properly. A checklist item long enough to wrap now continues under its own text instead of sliding back beneath the checkbox. And on Linux the whole list could appear pushed strangely far in — twice as far when nested — because the rule holding it flush relied on a stylesheet feature some Linux web engines don't support. It no longer does.",
      "Cards have a grip. Widget headers on the pinboard now show a small dot grid where you can reliably grab the card to drag it — beside the ingest button on a note, beside the pop-out on a card inside a folder. The whole header always dragged; there was just nothing showing you where to aim.",
      "Timelines can hide what's already passed. A “Past” checkbox on each timeline card tucks away entries whose date has gone by, the way the project panel's “Completed” checkbox hides finished milestones. Off means hidden, and the choice is remembered.",
      "Cards at the right edge stay where you put them. Switching away from the Pinboard and back could fling a card that sat flush against the right edge off to a default spot near the top-left. The board re-measures its width on every visit, and a scrollbar appearing or disappearing was enough to make that card look like it no longer fitted. Now it slides in by the cell or two it needs, keeping its place and its size.",
      "A few dropdowns now match everywhere else. Four menus — the backup account and frequency, the map's cohesion, and Teach's merge target — had been built by hand and missed a fix that keeps dropdown text from being clipped on Linux. They now use the same control as the rest of the app.",
    ],
  },
  {
    version: "3.86.0-alpha",
    date: "2026-07-26",
    highlights: [
      "Smart sort is finally smart. Sorting your Focus projects by “Smart” now ranks them the way you'd actually triage: what's due soon first — and within that, by the actual date, so something due today can no longer sit below something due next week — then by priority, then by whatever you touched most recently. Priority was never being considered at all before.",
      "The Focus tab uses your whole screen. The side-by-side layout no longer stops short at a fixed width, so the two columns fill the window instead of leaving wide empty margins, and the divider between them runs the full height. The stacked layout keeps its comfortable reading width.",
      "Each project remembers how you like its milestones ordered. The sort was one shared choice across every project, which suited none of them — a hand-ordered plan and a deadline-driven one want opposite things. Whatever you'd already chosen carries over as the starting point.",
      "Project cards are quieter, and “Suggest attributes” is where it belongs. The size chip is gone — the Quick win badge already says it — and the AI suggestion has moved from a button covering your whole project list into each project's own Triage panel. One click now asks about one project instead of quietly starting a request for every project you have.",
      "Your calendars are listed in the sidebar. Instead of hiding behind a “Calendars 5/9” button in the header, every calendar is shown inline at the bottom of the left sidebar while the Calendar tab is open, grouped by account and foldable, sitting just above today's briefing. Calendars you haven't chosen to sync are left out, since they can never show an event anyway.",
      "The Refresh button says when it last worked. The calendar's last sync time now rides on the button itself — a clock time if it synced today, the date if it was earlier, and the exact moment in the tooltip — so you can tell at a glance whether pressing it is worth it. The separate “Read-only” tag is gone; it never told you anything the rest of the page didn't.",
      "“Shared with me” says who shared it, and can be sorted. Each shared item now shows who sent it, and you can order the list by most-recently-shared or by name — Google returns them in no order at all, which is unhelpful once there are more than a few. PM also notices when someone stops sharing something: those files are kept findable and marked as no longer reachable, instead of sitting there looking current forever.",
      "An Open PM button on the floating briefing. The always-on-top briefing window can now bring the main window to the front, so you don't have to go hunting for it. The briefing stays where it is.",
      "You can decide whether to see which AI is in use. The models block at the bottom of the sidebar used to be governed entirely by your Depth preset — invisible on Minimal, always on otherwise. It's now a switch in Settings → General, so you can watch which model is answering on a pared-back setup, or put it away on a busy one. It's a readout only; hiding it changes nothing about which model runs.",
    ],
  },
  {
    version: "3.85.3-alpha",
    date: "2026-07-26",
    highlights: [
      "On Linux, you can set your working hours again. The start and end times on the calendar's Work and Day buttons were built on a control your web engine doesn't provide, so on Linux they fell back to a plain text box that undid every keystroke — you could never get a second digit in, and there was no way to step the value either. They're now half-hour dropdowns that behave the same on every system, and they only offer times that leave a sensible window, so a choice can't quietly do nothing.",
      "Events in Focus open like events in the calendar. Clicking one in the Upcoming card — in either the list or the day grid — now brings up the same detail panel the Calendar tab shows. That grid also honours the calendars you've hidden and the ones you've marked quiet, which it was ignoring, so a calendar you've silenced everywhere no longer keeps turning up there.",
      "Long event titles wrap instead of being cut off. In the agenda lists and on the Focus project cards a title now folds onto a second line rather than ending in an ellipsis. The day and month grids still trim, because there a card is only as tall as the event is long.",
      "Settings remember things they were forgetting. “Save a copy of dropped photos in the vault” now stays ticked — it was being reset every time you left the Documents tab, which also meant photos dropped afterwards were quietly not copied. Settings also say plainly that changes save as you make them, and the one control that doesn't — the backup schedule — now tells you when it's holding something unsaved.",
      "Importing your AI memory shows you what's happening. The Import button used to dim to a ghost of itself and change its own label, which read as a second, greyed-out button. It's now replaced by a progress bar while the work runs, the paste box is tall enough to read what you pasted, and screen readers are told when it starts and finishes.",
      "Trimming old backups tells you what it did. “Delete oldest, keep 5” did its job but said nothing at all — success was silent and failures appeared far up the page, out of sight. It now reports how many it moved to the trash, right where you clicked, and says so when there was nothing to trim.",
      "A rebuild no longer looks dead when you come back to it. Leaving the Documents tab mid-rebuild used to empty the activity list and blank the whole panel for several seconds, even though the timer kept running. The recent files are kept now, and the progress bar reappears immediately. If a rebuild finished while you were away, you'll see how it went.",
      "Google Drive stops crying wolf. An account whose last sync didn't finish was labelled “unreachable”, as though it were gone, and hiding the very controls you'd use to narrow down what was failing. It now says the sync failed, explains that Sync now retries, and leaves the folder and shared-drive chooser available. A failure listing shared drives no longer takes the My Drive controls down with it.",
      "Google Sheets: clearer about what it actually needs. Reconnecting for Sheets is a one-time thing — Google can't add a permission to a sign-in you've already given — and PM now says so instead of leaving it looking like a recurring chore. It also no longer tells you to reconnect when the real problem is that the Sheets API was never switched on for your Google Cloud project, which reconnecting can't fix; the setup guide now lists that step. And a routine token refresh can no longer make PM forget a permission you'd already granted.",
      "The briefing fills its window. Making the always-on-top briefing window or the floating panel taller grew the frame but not the text, leaving a blank strip below “Updated”. The text now takes the room you give it.",
      "Removing an alternative name in Teach is something you can see. The × on each “also known as” was invisible until you happened to hover exactly over it, so it looked like clicking did nothing. It's always visible now, with a proper click target. (The removal itself was working.)",
    ],
  },
  {
    version: "3.85.2-alpha",
    release: true,
    date: "2026-07-26",
    highlights: [
      "PM 3.85.2 rolls up everything since the last release into one update. Here's the tour at a glance — every line below has its full story in the entries that follow. There's nothing you need to do after updating: where this release repairs something, PM repairs it itself, quietly, on the next time it opens your vault.",
      "PM has a proper Accessibility tab. Scale every piece of text up or down, turn animations off regardless of what your device says, and switch the whole interface to Atkinson Hyperlegible — a typeface designed to make letters easy to tell apart. Alongside it: a Density control that sets how large controls and their click targets are, a Contrast control (AA, meeting the recommended 4.5:1, or High for AAA), and a colour-blind-safe palette that re-colours the things that carry meaning — calendar sources, map nodes, status badges — to an Okabe-Ito set, backing each calendar's dot with a distinct shape as well as a colour. Underneath it all, much more of PM now works from the keyboard and speaks properly to a screen reader: focus stays inside a dialog and returns where it started, a visible outline follows you as you Tab, and progress reads as “3 of 10” rather than a bare percentage.",
      "Today's briefing can follow you around, and keeps itself up to date. Put it at the bottom of the sidebar, in a small floating panel you can drag, or in an always-on-top window that stays in view while you work somewhere else — and PM can now sit in your system tray, menu bar or panel, so today is one click away without hunting for the window. The briefing re-checks itself when you open PM, once an hour, and within a minute of anything behind it changing; it only spends AI when something genuinely moved, so a quiet hour costs nothing.",
      "The Focus tab is yours to arrange. It now uses the width of your screen, with a divider you can drag between the two columns, and a Panels button to switch the briefing, focus box, Upcoming and projects list on or off. Upcoming can show a day-by-day hour grid instead of a list, framed to your working hours. Chat became Chats, with your projects and your global chats as two foldable lists that stay how you leave them.",
      "Every calendar event now opens. Click any event — not just your own milestones and notes — and a pop-up shows everything PM has synced: which calendar, busy or free, the location, guests and organiser, a video-call link, whether it repeats, and the full description, with buttons through to Google or Outlook, the linked project, or the Pinboard. PM now pulls those richer details from Google, Outlook and subscribed calendars, so existing calendars fill them in on their next sync. Timed events carry a tint of their calendar's colour, and finished events are gently greyed.",
      "Local AI got a lot better at sizing your machine. PM now reads the real memory on a dedicated AMD or Intel card on Windows instead of falling back to system RAM, looks up your card's actual memory bandwidth for its speed estimates, and sizes each model two ways where it helps — the highest-quality setup, and a faster one that fits inside your GPU. It also finds models you've already downloaded even when nothing is running them, tells you when one of them would suit your machine better than what you have assigned, and recognises far more file formats so fewer models come back as “can't estimate this”.",
      "Sorting a big import is quicker, cheaper, and no longer a queue. Approve each document the moment its suggestion is ready rather than waiting for the batch; file the rest of a folder the same way in one click; and PM now works its suggestions out in the background as soon as a sync finishes, asks about several documents per request instead of one at a time, and remembers what it suggested so closing PM never means paying for the same answer twice. AI suggestions are now something you turn on, not a requirement — when they're off or unavailable, Review tells you why and lets you file it yourself.",
      "Google Drive reaches the files people share with you. “Shared with me” is a separate place in Drive that PM couldn't see before — open a Google account under Connectors, turn it on, and pick the files or folders you want; a folder brings its contents, shortcuts are followed, and a file shared with two of your accounts is indexed once. You can also back up on demand to a connected Proton Drive or Google Drive, and PM offers to tidy a destination that's holding more backups than your limit.",
      "Fixes worth naming. Review's AI suggestions now actually fill the fields in — a blank row was being approved as Unsorted and recorded as you overruling the AI. Filing a chat no longer strips the markers that say “this is a conversation”, and PM repairs any that were damaged on its own. Changing your vault passphrase no longer loses how a cloud file was filed. Google Drive ingestion works again after Google rejected one outdated field name. Closing PM's window really does close PM. On Windows, connecting a Google account no longer hits a keychain size limit; on a Mac, PM asks for keychain permission once at startup rather than once per secret. Linux gets a Debian/Ubuntu .deb installer, and a package install is now told to reinstall to update rather than quietly failing to.",
    ],
  },
  {
    version: "3.85.1-alpha",
    date: "2026-07-26",
    highlights: [
      "Under-the-hood tidying: routine updates to two of the building blocks PM's own workshop is made from, bundled together. Nothing changes in how PM looks or works.",
    ],
  },
  {
    version: "3.85.0-alpha",
    date: "2026-07-26",
    highlights: [
      "The “Already downloaded” list in Settings > Local AI now says what you can actually do with what it found. Everything in that list is, by definition, a model nothing is currently running -- which also means PM can't use it yet. That was never said anywhere, so a model could sit there looking ready while never turning up in the Assign roles dropdown just above it. The list now explains it up front: load one in the app you downloaded it with, and it appears as something you can assign. If you haven't connected a server at all yet, it tells you to do that first instead.",
      "PM can size a lot more of the models you already have. Its table of quantization formats -- the compression a model file was saved with -- knew eleven of them, so a file in any of the older or less common ones (Q4_0, Q3_K_L, IQ4_NL, or a full-precision F16) came back as “can't estimate this”, even though PM had already measured that exact file's size on your disk. It knows twenty-one now. When it does still meet one it doesn't recognise, it names the format instead of implying it couldn't read your file -- those are two different problems and only one of them is yours.",
      "Settings > AI Models now says where the other half of the choice lives. The two roles on that page are your cloud models, and the very same roles can run on your own machine instead -- but that's set up in the Local AI tab, and nothing on the page mentioned it. There's now a line under the model lists that says so, with a button that takes you straight there.",
    ],
  },
  {
    version: "3.84.0-alpha",
    date: "2026-07-26",
    highlights: [
      "PM now tells you when a local model would suit your machine better than the one you're running, instead of waiting for you to go and look. A small dot appears next to Settings in the sidebar and next to the Local AI tab inside it, and the tab itself explains what it found -- for example that a model you've already downloaded is a better match than the one you have assigned. Dismiss it and every dot disappears at once.",
      "It is deliberately hard to annoy you. It only speaks up about a model your machine can comfortably run, only when it's a real step up rather than a rounding difference, and it measures against the largest model you already use -- so running a big model for chat and a small one for background work won't get you nagged about the small one. A model you've already got on disk always wins over one you'd have to download.",
      "You control how often it looks, from the Local AI tab: when PM's model list is updated (the default), weekly, monthly, or never. Choosing “never” turns it off entirely without hiding the setting that turns it back on.",
      "Under the hood this also fixes a setting that had never actually been saved: PM recorded how often to re-check and which model list it had last looked at, but nothing ever wrote either of them down. Nothing had surfaced those values yet, so nothing was visibly wrong -- but they had to work before any of the above could.",
    ],
  },
  {
    version: "3.83.0-alpha",
    date: "2026-07-26",
    highlights: [
      "Settings > Local AI now finds models you've already downloaded, even when nothing is running them. Until now PM could only see models a connected server was actively serving, so anything you'd downloaded and not loaded was invisible -- and PM would happily suggest you download a model already sitting on your disk. There's a new “Already downloaded” section that lists what it found, sized against your machine like everything else, so you can see at a glance which of them your computer can comfortably run.",
      "It looks in the places Ollama, LM Studio and Hugging Face actually keep their models, including the non-obvious ones -- a relocated Ollama store, LM Studio's portable-install pointer, the older cache folders. If you keep models somewhere else entirely, there's an “Also look in a folder…” button.",
      "What it reads is deliberately narrow: file names and sizes, nothing more. PM never opens a model file's contents, never writes anything while looking, and none of it leaves your device. A model it doesn't recognise is listed with an honest “can't estimate this” instead of a guess, and a half-finished download is left out rather than offered as something you can run.",
      "The Local AI tab is also clearer about what PM works with. The list of supported servers -- Ollama, LM Studio, llama-server, and any other server that speaks the OpenAI API -- is now stated plainly at the top of the endpoint section, whether or not you already have one connected, along with the reminder that PM connects to a server you run and never installs or starts one itself.",
    ],
  },
  {
    version: "3.82.0-alpha",
    date: "2026-07-26",
    highlights: [
      "If you run PM on Linux with an Intel Arc graphics card, Settings > Local AI can now see how much memory that card actually has. Until now PM could read the memory of NVIDIA and AMD cards but not Intel ones, so an Arc card fell back to being sized against your system RAM. That was honest, but it meant PM never offered you the faster “runs entirely on the graphics card” option -- even when your card had plenty of room for it. PM now asks the graphics driver directly, so a discrete Arc gets sized on its own memory like every other card.",
      "Nothing changes on Windows or a Mac, or on a machine with built-in Intel graphics: a built-in chip shares your system memory rather than having its own, and PM keeps saying so rather than inventing a number. As always, if the reading can't be taken for any reason, PM sizes on system RAM exactly as it did before -- it never guesses.",
      "One piece of honesty about this one: it is written from the Linux graphics driver's own documentation, not tested against a real Arc-on-Linux machine, because there isn't one to test on here. It's built so that if anything goes wrong it simply falls back to the old behaviour. If you have that setup and PM still doesn't pick your card up, that's worth reporting.",
    ],
  },
  {
    version: "3.81.3-alpha",
    date: "2026-07-26",
    highlights: [
      "A safety readout in Developer mode was lying, and it could only ever lie in the reassuring-to-alarming direction. PM processes files you feed it inside a sealed-off worker that is meant to have no way onto the network, and Developer mode has a self-test that proves it: one line for direct connections, one for DNS lookups. The DNS line was reading a result that never actually reached it, so it announced “not blocked” on every machine, every time, no matter what the test found. It now reports what the worker really came back with -- which, checked on Windows, is that both are properly blocked.",
      "In the same panel, the Confinement line no longer contradicts the test sitting underneath it. The worker only starts when there's work to do, so before then the line honestly reads “worker not started yet” -- but running the self-test is itself work, and the line used to keep saying that afterwards, directly above proof the worker had started and been refused a connection. It now re-reads once the test finishes.",
      "Both of these live under Settings > Developer mode, so if you've never turned that on, nothing you use has changed.",
    ],
  },
  {
    version: "3.81.2-alpha",
    date: "2026-07-26",
    highlights: [
      "An important fix for anyone who has chats saved. Filing a chat -- approving it in Review, changing its project, or renaming or merging a project that owns chats -- was quietly stripping the markers that tell PM “this file is a conversation”. Nothing looked wrong at the time, and nothing was: the damage only showed up the next time you re-indexed, when the conversation would come back as an ordinary document. Its citations would stop reopening the chat at the right message, and PM's own past answers would start being treated as source material -- which is exactly what its answers are not.",
      "That is fixed at the root, so it can't happen again from any of those actions. PM also repairs itself: when it opens your vault it checks every chat and puts back anything that went missing, and it does the same check again before any re-index, so there is no window where re-indexing could make old damage permanent. Your conversations themselves were never at risk -- the messages are the real record and they were untouched throughout -- so nothing is lost, and there is nothing for you to do. No re-index, no re-sync, no prompt.",
      "If a chat had already been through a re-index, PM rebuilds its index entry properly this time, so jump-to-a-message citations come back on their own. That happens quietly in the background and costs nothing -- the work is done on your machine.",
      "Curious whether it found anything? Turn on Developer mode and use “Check chat identity” under the sidecar panel: it reports how many chats are intact, and repairs any that aren't.",
    ],
  },
  {
    version: "3.81.1-alpha",
    date: "2026-07-25",
    highlights: [
      "The big one: Review's AI suggestions now actually fill in the fields. If PM suggested a project, an importance and some tags while the Review tab was closed -- which is what happens after a cloud or folder sync -- or if you closed the app and reopened it, you'd come back to the AI's written reasoning sitting above a row that still said Unsorted, with no tags and no importance, and the only way to get them back was to press Re-propose and pay for the suggestion twice. The suggestions were there all along; the row just never read them. It does now, and anything you'd edited by hand still wins over them.",
      "That one is worth a second sentence, because it was quietly costing you more than a repaint. Approving a row in that blank state filed the document as Unsorted -- and recorded it as you overruling the AI on all three fields, which is meant to be how PM learns your filing habits. If you approved things this way, those documents are in Unsorted and can be re-sorted from the Review tab.",
      "Progress bars now count from when the job really started. Leaving a tab mid-sync, mid-rebuild or mid-backup and coming back used to restart the elapsed timer at zero, so a twenty-minute index could read as one minute old. The timer now comes from the job itself, so it keeps counting no matter which screen you're on. (Visible at the Power depth setting.)",
      "The Projects and Global chats sections in the sidebar stay how you leave them. Folding one away now survives both switching tabs and restarting PM, instead of springing back open.",
      "The Focus tab's project column has a heading again -- the empty corner beside the Sort control now reads Projects, matching Today and Upcoming above it.",
      "Settings > General > Appearance: the “What these settings do” note was one short line that only told you the settings were saved. It now explains what each of System, Mode, Depth, Accent and Text size actually changes -- particularly Depth, which had no explanation anywhere in the app.",
    ],
  },
  {
    version: "3.81.0-alpha",
    date: "2026-07-25",
    highlights: [
      "Two accessibility touch-ups, both invisible unless you go looking. First, the Text size control now reaches the last few spots that used to ignore it -- a scattering of small labels (badges, timestamps, counts) were pinned to a fixed pixel size and stayed put when you scaled text up; they now grow with everything else, so Large and XL are consistent throughout. Second, small icon-only buttons -- the little ✕, chevron, folder and trash controls dotted around the calendar, pinboard, reader and sidebar -- now have a comfortably larger click/tap area (at least 24px, more on the Comfortable density) even though the icon itself looks the same, so they're easier to hit. Nothing changes how PM looks at your current settings.",
    ],
  },
  {
    version: "3.80.0-alpha",
    date: "2026-07-25",
    highlights: [
      "Your daily briefing keeps itself up to date. It used to be rewritten only when you opened PM and it was already more than half a day old -- so a meeting that appeared in your calendar this morning, or a milestone you ticked off, could leave the briefing quietly describing a day that had moved on. PM now checks when you open it, once an hour while it's running, and within a minute of anything that feeds the briefing changing: a calendar sync bringing in new events, a milestone added, edited or completed, or a reminder marked done.",
      "Checking is not the same as rewriting, so this costs you almost nothing. PM compares the facts behind the briefing -- the deadlines, today's events, what's blocked, what's gone quiet -- against the ones it was written from, and only asks the model for new wording when something genuinely moved. An hour in which nothing changed uses no AI at all.",
      "The briefing can no longer contradict itself between windows. If the same briefing was open in more than one place -- the Focus tab, the sidebar, the floating window -- two refreshes could overlap and leave the older text stamped with the newer time. Refreshes are now handled one at a time, and whichever window regenerates it, all the others update to match.",
    ],
  },
  {
    version: "3.79.0-alpha",
    date: "2026-07-25",
    highlights: [
      "The colour-blind-safe palette now backs its colours with shapes. When the Accessibility tab's colour-blind option is on, each calendar's dot in the Month grid takes a distinct shape as well as a colour -- a circle, diamond, triangle, square, and so on -- and the calendars menu shows the same shape beside each name, so you can tell sources apart by shape even where two colours look alike (or in greyscale). It only changes those small source dots, and only when the option is on; everything else, including event and project labels, is untouched.",
    ],
  },
  {
    version: "3.78.0-alpha",
    date: "2026-07-25",
    highlights: [
      "Closing PM's window really does close PM again. A recent change added the little always-on-top briefing window behind the scenes, and because that window is built to hide rather than close, it quietly kept PM running in the background after you shut the main window -- with no tray icon to get back to it. PM now quits properly when you close it.",
      'Google Drive can be ingested again. Choosing folders, picking from "Shared with me", or simply syncing was failing with a 400 from Google about an "invalid field selection". PM had been asking Drive for two details by their older names, and Drive rejects the entire request over one unrecognised name rather than just skipping it. Fixed -- nothing about what PM stores or how it is organised changes.',
      'Setting the floating briefing to "Inside PM" no longer opens the always-on-top window as well. Both would appear at once until you changed the setting again.',
      'Both floating briefings now say "Briefing -- Today" at the top, and the always-on-top one has a close button. Closing it also switches the setting off, so it stays gone rather than reappearing next time PM starts.',
      "The Focus box appears when you switch it on, even before you have any projects. It was hidden until at least one project existed, which made the toggle look broken on a fresh install -- though asking a question or saving a preference has always worked from an empty PM.",
      'Long event names in Upcoming wrap onto a second line instead of being cut off with a "...". In a card that narrow, most real meeting titles were losing the part that told you what they were.',
      "Upcoming's Work and Day buttons each carry a small arrow now, exactly like the Calendar tab's, so you can set which hours they frame. These hours are Upcoming's own -- narrowing Work here to suit the small card leaves the full Calendar tab alone.",
      'The Focus tab\'s settings have moved out of Settings, because they were already on the Focus tab itself, next to what they change. The Layout and Upcoming controls in Settings > General > Focus are gone; the ones in the Focus tab header do the job (and the Layout one there always worked, which its Settings twin did not). "Reset Focus" still puts everything back.',
      'An event that is not linked to anything no longer offers to "Open in Pinboard". That button appeared on every event you clicked in the calendar, and went to the Pinboard whether or not the event had anything to do with it.',
      'Contrast and Density each drop their old below-standard option. "Legacy" contrast and "Compact" spacing existed only so an earlier update would not change the look of PM under anyone; nobody was choosing them on purpose, and they sat below the readability and target-size levels PM aims for. Everyone is now on AA contrast and Standard spacing, with High and Comfortable still there if you want more.',
      'Importing the same AI memory twice no longer files everything a second time. PM now tells the import what it already knows so it can skip it, and separately compares meaning rather than exact wording -- so "based in Guildford" and "Based in Guildford." are recognised as one thing, not two.',
      "The Colour-blind-safe palette explains itself better. It re-colours the things that carry meaning -- status badges, Map nodes, calendar colours -- and deliberately leaves your chosen theme alone, which is why switching it on can look like nothing happened.",
      'The Focus tab\'s two columns now have a divider you can drag, so you can give the project list more room or less. It starts at an even split, neither side can be squeezed away to nothing, and where you leave it is remembered. Double-click the divider for an even split again, or use "Reset Focus" in Settings.',
      'Choosing which "Shared with me" items to sync no longer runs off the page. Past a handful of items the list becomes its own scrolling box with a count above it, so the rest of the connector\'s settings -- including Save -- stay where you can reach them.',
      "The section links under a Settings tab now flash the section they point at. They scrolled to it before, which looked like nothing at all on any tab short enough to fit on screen. Related: on those short tabs the last link no longer lights up as though you had selected it.",
    ],
  },
  {
    version: "3.77.2-alpha",
    date: "2026-07-25",
    highlights: [
      "Switching the Upcoming grid between Work and Day hours on the Focus tab now really does re-scale it. It was meant to all along -- a narrower window means taller hour rows -- but the rows had a minimum height chosen for the full-size Calendar tab, and in a card that small both windows bottomed out at it and came out looking the same, so switching seemed to do nothing but jump the scroll. The rows now stretch to fill the card.",
      "The 24h option has gone from that grid: in a card that size a whole day cannot show a readable event. Nothing is out of reach -- the grid still scrolls through every hour -- and the Calendar tab keeps all three windows.",
      "How many days Upcoming shows now sits right beside those buttons, instead of only in Settings where it was easy to miss. Both places still work and stay in step.",
      "The Accessibility tab now carries the familiar ringed figure with open arms rather than a seated one. That tab is text size, motion, contrast and spacing -- things anyone might want -- and the symbol should say so.",
    ],
  },
  {
    version: "3.77.1-alpha",
    date: "2026-07-25",
    highlights: [
      'Connecting a Google account on Windows could fail with a "keychain error" about a 2560-character limit. PM had recently moved to keeping all of its secrets inside a single Windows credential, and Windows caps how much one credential can hold -- so once the pile outgrew that cap, saving anything new was simply refused, and a signed-in account could have stopped refreshing too. On Windows and Linux each secret goes back to its own credential, comfortably inside the limit; anything already saved is moved across for you the next time PM starts, and you should not have to reconnect. Macs keep the single-credential layout, which is what stops them asking for keychain permission over and over.',
    ],
  },
  {
    version: "3.77.0-alpha",
    date: "2026-07-25",
    highlights: [
      "PM can now sit in your system tray, menu bar or panel. Switch it on in Settings > General > Focus and a small PM icon appears; click it (or right-click and pick \"Today's briefing\") to pop today up without hunting for PM's window. While the icon is on, closing PM's window leaves it running behind the icon rather than quitting, and Quit moves to the icon's menu -- switch the icon off and closing quits exactly as before.",
      'The floating briefing now has two strengths. "Inside PM" is the panel you already had; "Always on top" is a separate little window that floats over your other applications, so today stays in view while you work somewhere else. Both are still off unless you turn them on.',
      "A note for Linux: a left click on the tray icon does nothing, because desktops do not pass that click on to applications -- use the right-click menu. The icon itself shows on KDE Plasma, XFCE, Cinnamon and MATE; GNOME needs its AppIndicator extension installed first.",
    ],
  },
  {
    version: "3.76.0-alpha",
    date: "2026-07-25",
    highlights: [
      "You can now choose what the Focus tab shows. A new Panels button in its header lets you switch today's briefing, the focus box, Upcoming and the projects list on or off, so the tab holds what you actually use. One panel always stays visible so the page can never end up blank, and Reset Focus in Settings brings them all back.",
    ],
  },
  {
    version: "3.75.0-alpha",
    date: "2026-07-25",
    highlights: [
      "Today's briefing can now follow you around. Two new switches in Settings > General > Focus put it at the bottom of the sidebar, or in a small floating panel you can drag and resize that stays put as you move between tabs. Both are off unless you turn them on, and wherever it appears it is the same briefing -- refreshing any one of them updates them all.",
      "Every copy of the briefing now has a refresh button, and PM only ever generates one at a time, so two of them on screen can no longer both start the assistant writing at once.",
    ],
  },
  {
    version: "3.74.0-alpha",
    date: "2026-07-25",
    highlights: [
      'The Chat tab is now "Chats", and it shows your projects too. Two foldable lists sit in the sidebar: Projects, with the number of chats each one holds -- click through to open it -- and Global chats, the ones that belong to no project and search everything. Opening a project still works exactly as before.',
    ],
  },
  {
    version: "3.73.0-alpha",
    date: "2026-07-25",
    highlights: [
      "The sidebar now scrolls when it runs out of room. On a short window, or with larger text, the What's New and Settings buttons at the bottom could be pushed off the edge with no way to reach them -- now the tabs and your chat list scroll together and those buttons stay put.",
      "The Map tab now follows your chosen preset, like the Review and Teach tabs already do: hidden on Minimal, shown on Standard and Power. There is a new switch in Settings > General if you want it either way regardless.",
    ],
  },
  {
    version: "3.72.4-alpha",
    date: "2026-07-25",
    highlights: [
      "Answers in chat are now formatted properly. Bold text, bullet points, headings, tables and code all read the way the assistant meant them to, instead of showing the raw markers around them. Clicking a [1] still jumps to the source it came from. What you type stays exactly as you typed it.",
      "The same fix reaches the other places PM writes for you: your daily briefing on the Focus tab, the summary of what got condensed when a long chat is compressed, and the retrieval advice in Developer mode.",
    ],
  },
  {
    version: "3.72.3-alpha",
    date: "2026-07-25",
    highlights: [
      "A developer-workshop tidy-up with no effect on the app: PM's own source code now states how its files should be stored, instead of leaving it to each computer's settings. Nothing about PM looks or behaves differently.",
    ],
  },
  {
    version: "3.72.2-alpha",
    date: "2026-07-25",
    highlights: [
      "Under-the-hood tidying: routine updates to four of the building blocks PM is made from, bundled together. Nothing changes in how PM looks or works.",
    ],
  },
  {
    version: "3.72.1-alpha",
    date: "2026-07-25",
    highlights: [
      "Changing your vault passphrase can no longer lose how a cloud file was filed. PM keeps a small encrypted file alongside your vault listing every cloud file it has indexed and where you filed it. That file is locked with a key derived from your passphrase, so changing the passphrase left it unreadable, and PM rebuilt it from the database -- which meant anything the list knew about but the database didn't was quietly dropped, permanently. PM now re-locks that list (and your project-name rules) with the new passphrase as part of the change, so nothing is rebuilt and nothing is lost. If you've never changed your passphrase, you were never affected.",
    ],
  },
  {
    version: "3.72.0-alpha",
    date: "2026-07-25",
    highlights: [
      'The Map remembers where it put things. The "by project" arrangement was worked out from scratch every time you started PM -- the same documents, shuffled into the same shape, all over again. PM now saves the arrangement and reuses it, so the Map opens instantly and your documents are exactly where you left them. It re-works the layout only when something actually changes, like a document moving to a different project. (The "by meaning" arrangement already did this.)',
    ],
  },
  {
    version: "3.71.0-alpha",
    date: "2026-07-25",
    highlights: [
      "Filing suggestions are ready before you open Review. When a Drive, OneDrive, or folder sync finishes, PM now works out its suggestions for the new files straight away in the background, so opening Review shows them already there instead of starting the job while you wait. If Review is already open, they appear as they arrive. This only happens when AI suggestions are switched on, and it never asks about a document twice -- so it costs nothing extra, it just happens sooner.",
    ],
  },
  {
    version: "3.70.0-alpha",
    date: "2026-07-25",
    highlights: [
      "Sorting a big import is cheaper. Review used to ask the AI about your documents one at a time, re-sending the same instructions and project list with every single file -- so a hundred new documents meant a hundred near-identical requests. It now asks about several documents per request, which cuts most of that repetition. Suggestions still appear as they arrive (now in small groups rather than one by one), and if the AI ever loses track part-way through a group, PM quietly re-asks about those documents on their own rather than risking a suggestion landing on the wrong file.",
    ],
  },
  {
    version: "3.69.1-alpha",
    date: "2026-07-25",
    highlights: [
      "Filing suggestions now treat folder names as information, not instructions. When PM asks the AI where a file belongs, the folder the file came from was being written into PM's own instructions rather than passed alongside the document. Folders can be named anything -- and since PM can index folders other people share with you, that name isn't always yours -- so a folder named to look like an order could nudge a suggestion in a way you didn't intend. The folder now travels with the document as plain information, clearly marked as something to read rather than obey. Your suggestions are unchanged, and there's a bonus: PM can now reuse far more of each request, so a big import should cost less to sort.",
    ],
  },
  {
    version: "3.69.0-alpha",
    date: "2026-07-25",
    highlights: [
      "The Accessibility tab gains a Contrast control -- Legacy, AA, or High -- completing the accessibility set. A fresh install now defaults to AA, which meets the recommended 4.5:1 for body text; in practice PM's text was already almost there, so the only real change is that the very faintest label text is nudged a touch clearer. High goes all the way to AAA (7:1) and also firms up hint text and borders for maximum legibility. As with density, if you've been using PM already you keep the original Legacy ramp untouched -- switch to AA or High whenever you like, or Reset the tab to the compliant default. A built-in audit now checks every theme against these contrast targets so they can't quietly regress.",
    ],
  },
  {
    version: "3.68.0-alpha",
    date: "2026-07-25",
    highlights: [
      "The Accessibility tab gains a colour-blind-safe palette. Turn it on and the colours PM uses to tell things apart -- project graph nodes, calendar sources, and the status colours (due, blocked, and so on) -- switch to an Okabe-Ito set chosen to stay distinct under the common types of colour vision, including the usual red/green confusion. It's opt-in and changes only those category colours; your theme, text, and icons are untouched. (Distinct dot shapes to back up the colours are coming next.)",
    ],
  },
  {
    version: "3.67.0-alpha",
    date: "2026-07-25",
    highlights: [
      "The Accessibility tab gains a Density control -- Compact, Standard, or Comfortable -- that sets how large PM's controls and their tap/click targets are. Standard meets the recommended 24px minimum and is the default on a fresh install; Comfortable grows targets to 44px, which helps when precise clicking is hard. If you've been using PM already, nothing moves: you keep the original Compact spacing until you choose otherwise (or Reset the tab to the roomier default). Alongside it, every on/off switch in Settings now shares one consistent control, and the window buttons up top have larger, easier-to-hit areas -- all without changing how PM looks at your current setting.",
    ],
  },
  {
    version: "3.66.0-alpha",
    date: "2026-07-25",
    highlights: [
      "PM has a new Accessibility tab in Settings, with its first set of options. You can scale all of PM's text up or down (Small to XL) -- the same “Text size” control also sits under Appearance, since it's handy for everyone. You can turn animations off regardless of your device's motion setting. And you can switch the whole interface to Atkinson Hyperlegible, a typeface designed to be easy to read and to tell letters apart (numbers and code keep their usual font). These are opt-in and travel with your data -- nothing changes until you turn something on, and each has a Reset. More options are on the way to this tab: larger touch targets, colour-blind-friendly palettes, and a high-contrast theme.",
    ],
  },
  {
    version: "3.65.0-alpha",
    date: "2026-07-25",
    highlights: [
      "More of PM now works with the keyboard and with screen readers. The “drop files here” area, the file rows in Documents and in a project's file list, and pinboard notes can all be reached and opened with the keyboard now, not just the mouse. Screen readers announce the assistant's reply once it finishes, read progress bars as “3 of 10” rather than a bare percentage, and speak the voice-recording status. The vault passphrase box and its error messages are now properly labelled and announced. And the quick-jump palette (Ctrl/Cmd+K) describes its results list to assistive tech. It's all under the hood -- nothing looks different on screen.",
    ],
  },
  {
    version: "3.64.0-alpha",
    date: "2026-07-25",
    highlights: [
      "PM is starting to work better for keyboard and screen-reader users, and this is the groundwork. Open a dialog now and your keyboard focus stays inside it -- Tab moves between its own buttons instead of drifting onto the page behind -- and closing it puts focus back where you were. A clear outline follows the keyboard as you Tab around, so you can always see what's selected; it only appears for keyboard use, so nothing looks different when you're clicking with the mouse. Collapsed sections no longer quietly hold onto hidden focus, and screen readers can now announce which section of the sidebar you're on. The options you'll actually see -- text size, higher contrast, colour-blind-friendly palettes and reduced motion, gathered in a new Accessibility tab -- are coming in the next few updates.",
    ],
  },
  {
    version: "3.63.0-alpha",
    date: "2026-07-25",
    highlights: [
      "What's New now marks which versions were actual releases. Between releases there are lots of small interim versions (one per change we make), so it wasn't obvious which entries were the ones that actually shipped to you. Tagged releases now carry a “Release” badge here, so you can see at a glance when the last real release was and read every change that landed in each version along the way. (On a developer's own machine the newest version is usually an interim build with no badge — that's expected.)",
    ],
  },
  {
    version: "3.62.0-alpha",
    date: "2026-07-24",
    highlights: [
      "You can now undo a project-name decision in Teach, and PM protects your inbox. In the Teach tab, hover over any of a project's alternate names and a small × appears -- click it to remove that name, so it stops being treated as the project from now on. Honest note: this reverses the name and pulls any files still literally saved under that exact name back out into their own project, but it can't retroactively un-merge files an earlier merge already renamed to the project you kept (PM doesn't store that history) -- for those, restore a backup from before the merge. Separately, the special “Unsorted” inbox is now protected: it can't be merged into another project or renamed away, so a stray click can never sweep everything waiting to be sorted into the wrong place.",
    ],
  },
  {
    version: "3.61.0-alpha",
    date: "2026-07-24",
    highlights: [
      "Power users: progress bars now show how long the current task has been running. With the Power display density on, a live timer appears on the right of loading bars -- during ingests, re-indexing, syncs, backups and downloads -- next to the percentage or item count. It even ticks during the early “setting up” phase where there's no count yet, which is exactly when a long download benefits from it. One honest note: for a bar that reappears part-way through a task (say you reopened the Backup panel mid-upload), the timer counts from when it reappeared, not the original start.",
    ],
  },
  {
    version: "3.60.0-alpha",
    date: "2026-07-24",
    highlights: [
      "You can now bring your preferences over from another AI. If you've built up a “memory” in ChatGPT, Gemini or Claude, go to Settings → AI & Models → Import AI memory: copy the prompt there, run it in that assistant, and paste its reply back. PM turns it into tidy preference records and stages them in Teach → Preferences as suggestions -- nothing changes how PM works for you until you review each one and click Keep. Whatever you paste is treated as data, never as instructions.",
    ],
  },
  {
    version: "3.59.0-alpha",
    date: "2026-07-24",
    highlights: [
      "Click any event on the calendar to see its full details in a pop-up. Until now only your own milestones and pinboard notes responded to a click; now every event does. The pop-up shows everything PM has synced for it -- which calendar it's on, when, whether you're marked busy or free, the location, the guest list and organiser, a video-call link, whether it repeats, and the full description -- plus quick buttons to open it in its source calendar (Google / Outlook), jump to its linked project, or open the Pinboard. To make this useful, PM now pulls in those richer details (busy/free, guests, organiser, meeting links, recurrence) from Google, Outlook and subscribed (Apple / iCal) calendars, so the aggregated view finally carries what you'd see in the original app. Existing calendars fill in the new details on their next sync.",
    ],
  },
  {
    version: "3.58.1-alpha",
    date: "2026-07-24",
    highlights: [
      "The calendar reads more clearly at a glance. Timed events now carry a soft tint of their calendar's colour (the way all-day events already do), instead of a plain grey block that blended into the grid. The current day's column keeps its hour and day lines visible even on the quieter colour themes, where they used to wash out. And events that have already finished are gently greyed, so what's still ahead stands out from what's done.",
    ],
  },
  {
    version: "3.58.0-alpha",
    date: "2026-07-24",
    highlights: [
      "You can now back up on demand to a connected cloud, and PM helps keep that cloud tidy. On the Backup screen, each connected Proton Drive or Google Drive gets a \"Back up now\" button that uses your remembered passphrase and keeps only your most recent few backups (the keep-last-N you set) -- just like an automatic run, with no need to re-type the passphrase. And if a destination already holds more backups of this vault than your limit (say you had been saving manually for a while), PM shows a one-time note offering to either keep them all (raising the limit to match) or tidy up now by trashing the oldest down to your limit. It only ever counts and trims this vault's own backups, so a shared Drive folder is never touched by mistake, and nothing is hard-deleted -- trimmed backups go to the cloud's trash.",
    ],
  },
  {
    version: "3.57.1-alpha",
    date: "2026-07-24",
    highlights: [
      "Review now remembers the AI's suggestions between sessions. Before, if you closed PM with items still waiting in Review, re-opening it would ask the AI to suggest a project, tags and importance for the whole queue all over again -- quietly spending credits on work it had already done. PM now saves each suggestion as it arrives and reloads it on startup, so it only asks the model about genuinely new, un-suggested items. Re-propose still regenerates everything from scratch whenever you want a fresh take.",
    ],
  },
  {
    version: "3.57.0-alpha",
    date: "2026-07-24",
    highlights: [
      'macOS: PM now asks for your keychain permission once at startup instead of once for every saved secret. If you use PM on a Mac, you may have seen it show several login-password or "Always Allow" prompts in a row while it loaded — macOS asks separately for each stored item (your database key, API key, and calendar or cloud sign-ins), and PM used to keep each in its own keychain entry. It now stores them together in a single entry, so one "Always Allow" covers everything and the repeated prompts stop. Windows and Linux were never affected. One honest note: that single approval is a little broader than before — allowing it lets PM read all of its own saved secrets at once rather than one at a time — and none of them ever leave PM.',
    ],
  },
  {
    version: "3.56.2-alpha",
    date: "2026-07-24",
    highlights: [
      "Security housekeeping: updated a networking library in our dependencies to a patched version as a precaution. PM doesn't use the affected feature, so nothing changes for you — it just keeps our supply chain clear of known advisories.",
    ],
  },
  {
    version: "3.56.1-alpha",
    date: "2026-07-24",
    highlights: [
      "Linux: fixed a harmless-but-noisy crash report that popped up every time you closed PM (a WebKitGTK renderer shutdown quirk on some graphics drivers). Nothing was actually lost — your data is saved well before the app exits — but the app now shuts down cleanly instead of leaving a crash notification behind.",
    ],
  },
  {
    version: "3.56.0-alpha",
    date: "2026-07-24",
    highlights: [
      'Google Drive now indexes the files and folders people share with you. "Shared with me" is a separate place in Drive — apart from your own My Drive and from shared (Team) drives — and until now PM couldn\'t reach it, so anything a colleague shared straight to you (especially the contents of a shared folder) was invisible. Open a Google account under Settings → Connectors → Drive, turn on "Shared with me", and pick the specific files or folders you want: a folder brings its whole contents, shortcuts are followed to the real file, and if you connect two accounts on the same team a file shared with both is indexed just once. Everything stays read-only and index-only, like the rest of Drive.',
      'When you index only some folders of a Drive, you can now also include the loose files sitting in the drive\'s root — the ones not tucked inside any folder — with a checkbox under "Choose folders".',
    ],
  },
  {
    version: "3.55.0-alpha",
    date: "2026-07-24",
    highlights: [
      "Filed a document from a synced folder in Review? You can now file the rest of that folder the same way in one click. When the file came from a folder you're indexing (a Drive/OneDrive folder or a local folder), the row offers to \"apply this filing to the N other files from that folder\" — you pick the project, tick or untick any you don't want, and they're all filed at once. It's a plain, undo-able filing action (no AI needed), and it matches files by their actual folder, so two folders that happen to share a name are never mixed up.",
    ],
  },
  {
    version: "3.54.0-alpha",
    date: "2026-07-24",
    highlights: [
      "Filing documents in Review no longer waits on the AI. You can now approve each item the moment its suggestion is ready — a row's Approve button lights up as soon as that file is sorted, so you can clear the done ones while the rest are still being worked out, instead of waiting for the whole batch.",
      "AI suggestions in Review are now something you turn on, not a requirement. On a fresh install they start off, with a banner offering to switch them on — a real help when you're importing a lot — and you can also toggle them under Settings → AI & Models. When they're on but can't run, Review tells you why (no model linked, no credits, a local endpoint that's unreachable) and lets you set the project, tags and importance yourself and approve. The AI is always a help, never a gate.",
    ],
  },
  {
    version: "3.53.0-alpha",
    date: "2026-07-24",
    highlights: [
      "The Focus tab's “Upcoming” box can now show a short day-by-day calendar instead of a plain list. Switch between List and Days from the toggle at the top of the box (or under Settings → General → Focus). The Days view shows up to four days side by side as an hour grid — with the same Work / Day / 24h hour options as the calendar — and the ‹ › arrows step it a day forward or back, with a Today chip to jump straight back. It opens on today and stays where you leave it while you're on the tab. List stays the default.",
    ],
  },
  {
    version: "3.52.2-alpha",
    date: "2026-07-24",
    highlights: [
      "Milestones on the calendar now show which project they belong to. A milestone appears as an all-day item on its due date; it now reads as “milestone · project”, so two milestones with similar names in different projects are easy to tell apart at a glance.",
    ],
  },
  {
    version: "3.52.1-alpha",
    date: "2026-07-24",
    highlights: [
      "You can now make the left sidebar noticeably thinner. Its minimum width is now half what it was, so you can shrink it down and give the main view more room. As before, dragging the edge all the way to the left tucks the sidebar away behind the little reopen tab.",
    ],
  },
  {
    version: "3.52.0-alpha",
    date: "2026-07-24",
    highlights: [
      "Clicking a document on the memory map now opens it in the reading panel — the same one you get from the Documents tab — so you can read it right there instead of jumping to its whole project. The project name in that panel is now a link, so the project is still one click away. The map also redraws crisply now if you drag the window onto a screen with different scaling.",
    ],
  },
  {
    version: "3.51.0-alpha",
    date: "2026-07-24",
    highlights: [
      "A project's milestones now sort by deadline by default and open on the next one you haven't ticked off, with completed milestones tucked just above — scroll up to see what's done. A new \"Completed\" checkbox by the sort controls hides or shows the finished ones, and your sort choice is now remembered when you leave a project and come back. You can still switch to Manual to drag milestones into your own order.",
    ],
  },
  {
    version: "3.50.0-alpha",
    date: "2026-07-24",
    highlights: [
      "The Focus tab now uses the width of your screen. Your project list sits beside the daily briefing, quick actions and upcoming events, instead of stacked in one narrow column with empty space down both sides. On a narrow window it folds back to a single column. This split view is the new default — you can switch back to a stacked column from the toggle in the Focus header, or under Settings → General → Focus.",
    ],
  },
  {
    version: "3.49.3-alpha",
    date: "2026-07-24",
    highlights: [
      "The Pinboard now fits your app window instead of your whole screen. On Macs and Linux it could open larger than the window with scrollbars down the side and along the bottom, because it was sized to the full display. It's now sized to the window: a board imported from a wider screen tidies itself to fit when it opens, and it only grows downward (never sideways) when it holds more than fits — so it never spills past the edges. Moving notes into a folder shrinks it back. (Windows already looked right.)",
    ],
  },
  {
    version: "3.49.2-alpha",
    date: "2026-07-24",
    highlights: [
      "The Sort dropdown on the Focus tab now shows its text centered and in full on Linux. It was rendering too low, with the bottom of the letters clipped off; the control is now sized so the text sits properly on every system. (Windows and Mac already looked right.)",
    ],
  },
  {
    version: "3.49.1-alpha",
    date: "2026-07-24",
    highlights: [
      "The hour gridlines in the week and day calendar views now render cleanly on Retina Macs. Some of the faint grey hour lines could previously come out uneven or drop out entirely there; PM now draws each as a crisp single-pixel line so they all show up. (Windows and Linux were unaffected.)",
    ],
  },
  {
    version: "3.49.0-alpha",
    date: "2026-07-23",
    highlights: [
      "The Local AI speed estimates now match your actual graphics card. Before, PM assumed the same middle-of-the-road speed for every dedicated GPU, so a fast card and a modest one showed the same tokens-per-second guess. PM now looks up your card's real memory bandwidth — the thing that most governs how fast a local model replies — so the estimate reflects what you actually have (a high-end card reads its memory several times quicker than an entry-level one). Where one card name ships in two memory sizes that run at different speeds, PM uses your card's memory to tell them apart. If your exact card isn't recognised, PM says so and falls back to the old general estimate rather than guess. This only sharpens the speed number on the model cards — it never changes which models fit.",
    ],
  },
  {
    version: "3.48.0-alpha",
    date: "2026-07-23",
    highlights: [
      "Local AI is smarter about fitting bigger models on your machine. A model keeps a running memory of the conversation as it goes (its “KV cache”), and until now PM always sized that at full precision — so a model that was close to fitting had to either drop to a more compressed, lower-quality version or shrink how much text it can consider at once. PM now quietly compresses that running memory to a near-lossless half-size setting when — and only when — doing so lets the model keep its full context or a higher-quality version instead. Cards that use it show a small “q8_0 KV” note so you can see what changed, and anything that already fit is untouched. It helps most on computers with a graphics card, where it can now fit setups that used to spill over.",
    ],
  },
  {
    version: "3.47.0-alpha",
    date: "2026-07-23",
    highlights: [
      "Local AI now reads the real graphics memory on more computers. On Windows, PM couldn't tell how much memory was on a dedicated AMD or Intel graphics card bigger than 4 GB — Windows reports that figure in a field that maxes out at 4 GB — so those cards quietly fell back to sizing models on your system memory and never got the faster “on your GPU” setup. PM now reads the true amount straight from the graphics system, so an Intel Arc or a larger Radeon card gets the same two-way sizing an NVIDIA card already did. NVIDIA cards, smaller cards, integrated graphics and Macs are unaffected. (Reading a dedicated Intel card's memory on Linux is still to come — it needs a different route there.)",
    ],
  },
  {
    version: "3.46.1-alpha",
    date: "2026-07-23",
    highlights: [
      "The new two-way local-model sizing now treats integrated graphics honestly. On a computer whose graphics share system memory - Apple Silicon, or an AMD/Intel integrated GPU - PM no longer offers a lighter “faster on your GPU” setup that wouldn't actually be faster (there's no separate graphics memory to win from). Those machines simply see the single best setup, as they should. Computers with a real dedicated graphics card are unaffected.",
    ],
  },
  {
    version: "3.46.0-alpha",
    date: "2026-07-23",
    highlights: [
      "Local AI now sizes each model two ways on a computer with a graphics card: the highest-quality setup that fits your memory, and a faster, lighter one that fits inside your GPU — usually a smaller, more compressed version at a shorter context, but replies stream far quicker. The Workbench shows both side by side with the speed difference spelled out, so you can choose. Before, PM only ever suggested the highest-quality setup, even when a lighter one would have run many times faster on your card. If a model already fits your graphics card, or your Mac shares one pool of memory, you'll just see the single best setup — and with no dedicated graphics card, nothing changes. PM still never switches for you.",
    ],
  },
  {
    version: "3.45.1-alpha",
    date: "2026-07-23",
    highlights: [
      "Restoring a backup onto a new computer settles in properly now. When you bring a backup to a fresh machine and switch to it, PM moves the restored vault into place as your everyday vault — instead of leaving it parked in a side folder you had to shuffle files out of by hand. And if the backup was a passphrase-protected “shareable” vault, PM asks whether to make it private to this device (the usual choice — sharing is something you set up per computer, and none of that setup travels in a backup) or keep the passphrase, and it no longer carries the original computer's owner tag across. Restoring onto a computer that already has a vault, or restoring to recover the same machine, is unchanged.",
    ],
  },
  {
    version: "3.45.0-alpha",
    date: "2026-07-23",
    highlights: [
      "Linux gets a Debian/Ubuntu installer. Alongside the AppImage and the Fedora .rpm, PM now ships a .deb — so Debian, Ubuntu, Mint and friends can install it with a single `sudo apt install ./pm_*.deb` and get PM in their app menu, no fuss. And if you installed PM from a package (rpm or deb), the update banner now points you to reinstall the new package instead of quietly trying — and failing — to update itself the way only the AppImage can. Not on Linux, or on the AppImage? Nothing changes.",
    ],
  },
  {
    version: "3.44.2-alpha",
    date: "2026-07-23",
    highlights: [
      "Turning a shared vault back into a private one now locks other accounts out of the folder more reliably. When you make a vault you'd shared on this PC private again, PM decrypts your notes back to plain files in place — so any Windows account you'd linked has to lose its folder access at that moment. PM now does that by reading the permissions actually on the folder and clearing every account but you, instead of leaning on a saved list of who to remove that could be out of date or tampered with. Your notes were always safe while shared; this tightens the hand-back to private. (macOS and Linux already worked this way, and nothing changes if you've never shared a vault.)",
    ],
  },
  {
    version: "3.44.1-alpha",
    release: true,
    date: "2026-07-23",
    highlights: [
      "PM 3.44.1 rolls up everything since the last release into one update. Here's the tour at a glance — every line below has its full story in the entries that follow.",
      "One optional thing after updating: if you chatted with PM before this update, open the Documents tab and click Rebuild once. PM no longer treats its own past answers as source material when it searches your notes, and that single Rebuild clears the older answers out of search so a stale reply can't quietly shape a new one. If you're new to PM, there's nothing you need to do.",
      "Run AI on your own machine — free, and private. A new Settings › Local AI tab scans your computer, recommends on-device models it can actually run (and roughly how fast), connects to a local model server like Ollama or LM Studio, and lets you send your chats — or just the behind-the-scenes work — to it, falling back to the cloud only when the local model isn't reachable. A model on your own machine means nothing leaves your device — and PM now shows you which model answered, and says so honestly when a reply came from the cloud instead. If you use Ollama, PM can download a recommended model straight into it.",
      "You can now start PM without an API key. On first run, choose a cloud provider, a model on your own device, or “set up AI later” — PM works either way, and tells you plainly when a feature needs an AI provider you haven't set up yet. Already using PM? Nothing changes.",
      "PM grounds its answers more carefully. It now measures how strong the best match to your question really is, and when nothing fits well it says so and answers from general knowledge rather than dressing up a weak guess as a fact from your notes. It also weighs the whole shortlist of passages (not just a rough top few) and reads each one's section heading, so the passage you actually meant is likelier to surface — and there's a “Save as note” under each answer to keep a good reply as a real, searchable note in your vault.",
      "The file-reader is now sealed off on every system. The helper that opens and converts your files runs with no way to reach the internet and can only see the file it's working on — not your vault, not the rest of your computer — first on Windows, and now on Linux and macOS too. So even a booby-trapped document can't use it to phone home or snoop around. It's an extra wall, not a gate: if the sandbox can't fully start, PM keeps working and reports a short code (like SBX-3101) you can quote in a bug report.",
      "Settings got a big tidy. Changes now save the moment you make them — the Save button is gone — the tabs are grouped and icon-labelled down the side with their sub-sections listed to jump straight to, and a small “Reset” appears next to anything you've moved off its default (with a “Reset to defaults” on each tab). Your API keys, search language and time zone are deliberately left untouched.",
      "Steadier under the hood. A rebuild now resumes where it left off instead of starting over, and search keeps working the whole way through; your passphrase is taken exactly as you type it; photos saved into a vault survive a passphrase change and come out in a plain-files export; “Remove PM data” now clears every vault key PM had cached, not just the current one; one corrupt photo can no longer freeze the document engine; and a chat reply that fails partway through is reported as a failure instead of being saved as though it were the answer. Your calendars also keep their list in sync on every sync and decide whether something is “today” in your own timezone.",
    ],
  },
  {
    version: "3.44.0-alpha",
    date: "2026-07-23",
    highlights: [
      "You can now start PM without an API key. On first run, choose a cloud provider (OpenRouter), a model running on your own device, or “Set up AI later” — PM works either way, and it tells you plainly when a feature needs an AI provider you haven’t set up yet. Already using PM? Nothing changes.",
    ],
  },
  {
    version: "3.43.0-alpha",
    date: "2026-07-23",
    highlights: [
      "When you run a local model, PM now shows you which one answered — and tells you honestly when a reply came from the cloud instead. A small “Local” line in the sidebar and a pill next to the message box show whether your local model is connected, resting, or unreachable, and a gentle note appears above the chat whenever a reply had to fall back to the cloud (because the local model was busy, still loading, or couldn’t be reached). If you don’t use a local model, nothing changes.",
    ],
  },
  {
    version: "3.42.0-alpha",
    date: "2026-07-23",
    highlights: [
      "Changed a setting and want it back the way it was? A small “Reset” now appears next to anything you’ve moved off its default, and each of the General, AI & Models, and Search tabs has a “Reset to defaults” that puts everything on it back at once — appearance, model choices, the memory map, re-ranking, and more. Your API keys, search language, and time zone are deliberately left untouched.",
    ],
  },
  {
    version: "3.41.1-alpha",
    date: "2026-07-23",
    highlights: [
      "Under-the-hood tidying of the Settings internals, with no change to how anything works: the backend code that stores your preferences moved into its own tidy home, and the on/off settings now share one small helper instead of each repeating the same steps. Purely groundwork — it makes the next batch of settings quicker and safer to add.",
    ],
  },
  {
    version: "3.41.0-alpha",
    date: "2026-07-23",
    highlights: [
      "Run AI on your own machine — free, and private. The new Settings → Local AI tab scans your computer, recommends on-device models it can actually run (and roughly how fast), connects to a local model server like Ollama or LM Studio, and lets you send your chats — or just the behind-the-scenes work — to it, falling back to the cloud only if the local model isn't reachable. A model on your own machine means nothing leaves your device. If you use Ollama, PM can download a recommended model straight into it.",
    ],
  },
  {
    version: "3.40.0-alpha",
    date: "2026-07-22",
    highlights: [
      "Settings is easier to get around: the tabs are now grouped and icon-labelled down the side, and the tab you're on lists its sub-sections right there — click one to jump straight to it, or just scroll and watch it keep pace. Everything's where it was, only quicker to find.",
    ],
  },
  {
    version: "3.39.0-alpha",
    date: "2026-07-22",
    highlights: [
      "Settings now save the moment you change them — the Save button is gone, so there's nothing to remember to click. Your API key shows a green 'Saved' the instant it's stored. (Groundwork, too, for adding new settings cleanly — like the on-device AI setup coming soon.)",
    ],
  },
  {
    version: "3.38.0-alpha",
    date: "2026-07-22",
    highlights: [
      "Under-the-hood groundwork for on-device AI: PM can now read your computer's memory, processor, and graphics card, and size a curated list of local models against it — working out the highest quality each one could run, and roughly how fast. Nothing to switch on yet; the setup screen that shows these recommendations comes next.",
    ],
  },
  {
    version: "3.37.1-alpha",
    date: "2026-07-21",
    highlights: [
      "Under-the-hood tidying, no visible change: when a live chat borrows your local model for a moment, the background jobs it interrupted (like chat summaries and titles) now wait a beat and retry on their own, instead of sitting out until much later. Also added internal notes so a fiddly database-upgrade footgun can't quietly bite us again.",
    ],
  },
  {
    version: "3.37.0-alpha",
    date: "2026-07-21",
    highlights: [
      "Under-the-hood: the local-AI engine is now live inside PM. It can talk to a local model server, hand a request to the cloud honestly when the local one is slow or unreachable (and tell you when it did), and count local and cloud usage separately. There's still no switch to flip — the Local AI setup screen that turns it on comes next.",
    ],
  },
  {
    version: "3.36.1-alpha",
    date: "2026-07-21",
    highlights: [
      "Under-the-hood tidying, no visible change: every AI request now flows through a single internal routing point. It behaves exactly as before today, and it's the groundwork that lets a local, on-device model slot in cleanly next.",
    ],
  },
  {
    version: "3.36.0-alpha",
    date: "2026-07-21",
    highlights: [
      "Under-the-hood groundwork for an upcoming option: running AI models locally, on your own machine, so your chats can stay entirely on your device. This release lays the plumbing only — there's nothing to switch on yet. The setup screens follow.",
    ],
  },
  {
    version: "3.35.1-alpha",
    date: "2026-07-21",
    highlights: [
      "Under-the-hood tidying. Your Usage & cost view now counts a slice of background AI work — chat summaries, titles, and a couple of others — that a stray database rule had been quietly dropping, so your totals read a little more complete. We also firmed up an internal code boundary and moved a shared building block to its proper home. Nothing changes in day-to-day use.",
    ],
  },
  {
    version: "3.35.0-alpha",
    date: "2026-07-20",
    highlights: [
      "The file-reader's protective sandbox now covers macOS too — completing the set alongside Windows and Linux. The helper that opens your files runs with no way to reach the internet (not even to look up a web address) and can only see the handful of folders it actually needs, never your vault or the rest of your home folder. As always it's an extra layer, not a gate: if the sandbox can't fully start, PM keeps working and reports a short code (like SBX-3101) you can quote in a bug report. Nothing changes in day-to-day use.",
    ],
  },
  {
    version: "3.34.0-alpha",
    date: "2026-07-20",
    highlights: [
      "The file-reader's protective sandbox now covers Linux too: the helper that opens your files runs with no way to reach the internet and can only see the handful of folders it actually needs — not your vault, not the rest of your home directory — the same wall Windows already had. On older Linux kernels that lack the newer 'Landlock' feature, the network is still blocked and PM says so honestly rather than pretending. As always it's an extra layer, not a gate: if the sandbox can't fully start, PM keeps working and reports a short code (like SBX-4105) you can quote in a bug report. Nothing changes in day-to-day use — macOS gets the same treatment next.",
    ],
  },
  {
    version: "3.33.1-alpha",
    date: "2026-07-20",
    highlights: [
      "If PM ever can't start the file-reader's sandbox, it now shows a short code (like SBX-2104) — and there's a plain-English list of what each code means (ERROR_CODES.md in the project, and Developer mode → Sidecar sandbox in the app). Popping that code into a bug report lets us pinpoint the problem fast. Nothing changes in normal use.",
    ],
  },
  {
    version: "3.33.0-alpha",
    date: "2026-07-20",
    highlights: [
      "Under-the-hood groundwork for bringing the file-reader's protective sandbox to Mac and Linux next: the shared machinery now lives in one place, and if the sandbox ever can't start it reports a precise code (like SBX-2104) instead of a vague message — so a one-line note is enough for us to pinpoint and fix it. Nothing changes in day-to-day use.",
    ],
  },
  {
    version: "3.32.0-alpha",
    date: "2026-07-19",
    highlights: [
      "Developer mode now shows, at a glance, whether the file-reading helper's Windows sandbox is actually switched on — and, in development builds, a one-click check that confirms it really can't reach the internet. This is purely a window onto the protection added last update; if you don't use Developer mode, nothing here changes for you.",
    ],
  },
  {
    version: "3.31.0-alpha",
    date: "2026-07-19",
    highlights: [
      "On Windows, the helper that opens and reads your files now runs inside a locked-down sandbox: it has no way to reach the internet, and it can only see the one file it's working on — not your notes, not the rest of your computer. So even a booby-trapped document can't use PM's file reader to phone home or snoop around. Everything works exactly as before; this is simply an extra wall, quietly added around the riskiest part of PM. (The AI models still download normally through a separate, sandbox-free step, and macOS and Linux get the same treatment next.)",
    ],
  },
  {
    version: "3.30.0-alpha",
    date: "2026-07-19",
    highlights: [
      "Under-the-hood tidying that also helps at uninstall time: PM now keeps its downloaded on-device AI models inside its own data folder, so removing PM cleans them up too — previously some were left behind in a temporary system folder. The first time you search or add something after this update, the main model may download once more into its new home; after that, everything runs on your device exactly as before.",
    ],
  },
  {
    version: "3.29.0-alpha",
    date: "2026-07-19",
    highlights: [
      "Under-the-hood security groundwork: the helper that opens and converts your files is being prepared to run completely sealed off from the internet, so that even a malicious file could never use it to reach out. As a first step, downloading PM's on-device AI models is now handled by a separate, short-lived step — leaving the file reader with no reason to touch the network. Day to day nothing changes: the models still download once on first use, then everything runs on your device as before.",
    ],
  },
  {
    version: "3.28.0-alpha",
    date: "2026-07-19",
    highlights: [
      "On a Mac, sharing a vault with other accounts on the same computer now locks the folder down to just the people you choose — the same extra protection PM already gave shared vaults on Windows and Linux. Other accounts signed in to that Mac can no longer reach the raw files; your notes were always encrypted, and this simply adds a lock on the folder on top. If you never share a vault, nothing changes for you.",
    ],
  },
  {
    version: "3.27.1-alpha",
    date: "2026-07-19",
    highlights: [
      "Fixed a Windows-only hiccup where the occasional web page saved in your Google Drive or OneDrive could fail to import. PM briefly collided with antivirus over the temporary copy it was reading, so that one page was skipped. It now gives each file its own name and waits out that momentary lock, so these pages index reliably. Everything else about how your cloud files sync is unchanged.",
    ],
  },
  {
    version: "3.27.0-alpha",
    date: "2026-07-18",
    highlights: [
      "PM now chooses the passages behind an answer more carefully. Until now its smart re-ranker only re-ordered a short list of notes that a first, rougher pass had already picked — so a strongly relevant note that the rough pass ranked just out of reach could never make it in. The re-ranker now weighs the whole shortlist, so those near-misses can surface. You should notice fewer “but I know I wrote that” moments on questions your files genuinely cover.",
    ],
  },
  {
    version: "3.26.1-alpha",
    date: "2026-07-18",
    highlights: [
      "Routine housekeeping: refreshed the behind-the-scenes libraries and build tools PM is built on to their latest releases. Nothing changes in how PM looks or works — this just keeps the foundations current and secure.",
    ],
  },
  {
    version: "3.26.0-alpha",
    date: "2026-07-18",
    highlights: [
      "The safety check from the last update is now switched on. When you ask PM about something your files don't really cover, it now notices that the closest match is weak, says so plainly, and answers from general knowledge instead of dressing up a shaky guess as a fact from your notes. Ask about something your files do cover and nothing changes — it answers and cites its sources exactly as before. (For the curious: turn on Developer mode and each answer shows the confidence score behind it, with a dial to adjust or switch off the check.)",
    ],
  },
  {
    version: "3.25.0-alpha",
    date: "2026-07-18",
    highlights: [
      "Groundwork for a new safety check on how PM grounds its answers. When PM searches your files for an answer, it now measures how strong the best match actually is — so that, once tuned, it can tell the difference between “your notes really cover this” and “nothing here fits,” and stop confidently answering from a weak match. The check is switched off by default while it's being calibrated, so nothing changes in how you chat today.",
    ],
  },
  {
    version: "3.24.0-alpha",
    date: "2026-07-18",
    highlights: [
      "PM no longer treats its own past answers as source material when it's answering you. It still keeps every conversation in full — nothing is deleted, and you can reread any chat — but when it looks for relevant background it now draws only on your own notes, files and messages, not on things it said earlier. That closes a subtle loop where an older, imperfect answer could quietly shape a new one. And since a reply is often worth keeping, there's now a \"Save as note\" button under each answer: one click files it as a real, searchable note in your vault, tagged with a small reminder that it came from a chat. (If you chatted before this update, press Rebuild once in Documents to clear the old answers out of search.)",
    ],
  },
  {
    version: "3.23.1-alpha",
    date: "2026-07-18",
    highlights: [
      "Another improvement to how PM picks which of your notes to read when answering. To rank the passages it finds, PM takes a second, closer look at each one — but that closer read was only seeing the passage text, not the section heading above it. So a section whose heading named exactly what you asked about could be passed over when its text happened to open with routine boilerplate. PM now shows that closer read the heading too, so the right section is more likely to rise to the top. Nothing looks different in everyday use.",
    ],
  },
  {
    version: "3.23.0-alpha",
    date: "2026-07-17",
    highlights: [
      'For anyone who likes to look under the hood: turn on Developer mode and each chat answer now carries a collapsed "Prompt sent to the API" panel — open it to see exactly what PM sent the model for that reply, its own instructions and the notes, calendar and reminders it drew on. The in-chat retrieval "Explain" tool moved into Developer mode alongside it. Leave Developer mode off and nothing about chat changes.',
    ],
  },
  {
    version: "3.22.5-alpha",
    date: "2026-07-17",
    highlights: [
      "Fixed pinboard notes running your lines together. When you write a note across several lines and then click away to read it back, PM now keeps the line breaks exactly where you put them instead of merging everything into one paragraph.",
    ],
  },
  {
    version: "3.22.4-alpha",
    date: "2026-07-17",
    highlights: [
      "A behind-the-scenes tweak to how PM chooses which of your notes to read when answering. When one long document — or a run of very similar passages — dominates the results, PM now makes room for other relevant sources instead of letting a single section crowd everything else out, so answers can draw on a wider slice of what you've saved. Nothing looks different in everyday use.",
    ],
  },
  {
    version: "3.22.3-alpha",
    date: "2026-07-17",
    highlights: [
      "More under-the-hood hardening, this time around chat. When you ask PM something, it draws on your relevant notes, calendar and reminders as background material — which PM now keeps cleanly separated from its own instructions, so text living inside your own documents can never be read as a command to the assistant. It's a safety boundary behind the scenes and doesn't change how you chat.",
    ],
  },
  {
    version: "3.22.2-alpha",
    date: "2026-07-17",
    highlights: [
      "Under-the-hood hardening of how PM handles file locations. When you import files, add a folder, save a backup or export, or move your vault, PM now double-checks the location is a real, sensible path before it touches your disk — an extra safety net you won't notice in everyday use. Choosing where to put a plaintext export, and pointing PM at the Proton Drive program, now open their picker from PM itself for the same reason. Nothing changes in how you use any of these.",
    ],
  },
  {
    version: "3.22.1-alpha",
    date: "2026-07-16",
    highlights: [
      "Renaming a project no longer empties its pinboard timeline. A timeline widget pinned to a project tracked it by name, so renaming (or merging) the project moved its milestones and left the widget pointing at a name that no longer existed — and it silently went blank. Renaming now carries those widgets across too, without touching anything else on your board.",
    ],
  },
  {
    version: "3.22.0-alpha",
    date: "2026-07-16",
    highlights: [
      "In a shared vault, connectors now clearly belong to the vault's owner. When you share a vault with another Windows account on your PC, syncing a connector like Google Drive or OneDrive relies on sign-in details that live in one account's private keychain — so a second account could never actually sync them, and trying just produced a confusing 'connection failed'. Now PM says so plainly: the vault's owner sets up the connectors, and everyone who shares the vault still sees everything that gets indexed. (Existing shared vaults are unaffected — this applies to vaults shared from this version onward.)",
    ],
  },
  {
    version: "3.21.0-alpha",
    date: "2026-07-16",
    highlights: [
      "Rebuilding your index no longer breaks links to your chats. When PM rebuilds its search index it re-processes every chat — and it used to give each rebuilt chat a brand-new internal identity, quietly breaking saved references to it (the 'jump to this chat' links, and the corrections PM had learned from it). Rebuilt chats now keep their identity, so those links survive — the same fix documents got in an earlier release.",
      "Cleaned up rare duplicate entries left over from indexing one shared Google Drive on two accounts. If you'd indexed the same shared drive from two connected accounts before PM learned to de-duplicate them, a leftover duplicate could linger. PM now merges away any duplicate whose filing matches — and where two copies were filed differently, it leaves both rather than guess which to keep and risk discarding your filing.",
    ],
  },
  {
    version: "3.20.0-alpha",
    date: "2026-07-16",
    highlights: [
      "Your calendar list now keeps itself in sync. Before, PM only learned which calendars an account had at the moment you connected it — so a calendar you created later never appeared, and one you deleted upstream kept failing every sync and pinning the whole account as 'unreachable'. Now each sync rechecks the list: new calendars show up on their own (untick any you don't want), and deleted ones are cleaned up — but only when PM has provably seen your whole list, so a dropped connection can never remove a calendar by mistake.",
    ],
  },
  {
    version: "3.19.10-alpha",
    date: "2026-07-16",
    highlights: [
      "PM now describes its cloud indexing accurately. The Google Drive and OneDrive help said PM keeps 'a searchable pointer and a short summary' — which undersold it: PM actually reads each file's full contents to build its search index, it just never keeps a copy of the file. The wording now matches what search can really do.",
      "The scheduled-backup help now describes what actually happens: a backup runs when PM is unlocked and idle with a passphrase set and nothing else syncing, and if a destination can't be reached right then it's simply retried next time — rather than promising an 'online' check only some destinations make.",
    ],
  },
  {
    version: "3.19.9-alpha",
    date: "2026-07-16",
    highlights: [
      "Help mode now answers everywhere it lights up. Four places highlighted as if they had an explanation and then showed nothing when you hovered them — including Remove PM data, the one place in Settings where knowing exactly what a button does matters most. They all have their explanation now, and the model picker's no longer claims you can search every model on OpenRouter (PM only lists the ones it can actually use).",
      "Times on the Focus tab now match the rest of PM instead of quietly using their own format.",
      "Under-the-hood tidying: our own description of what PM sends over the network now mentions syncing the cloud accounts you connect — it listed everything else and somehow not that.",
    ],
  },
  {
    version: "3.19.8-alpha",
    date: "2026-07-16",
    highlights: [
      "One bad photo can no longer freeze everything PM does with your files. A photo whose GPS data is corrupt produced a number that can't be written down — and because PM's document engine handles one job at a time, that single photo jammed the queue for half an hour: no importing, no search, no voice notes, no memory map, with nothing on screen to say why. Corrupt location data is now simply treated as no location, and any impossible number fails just that one job instead of the whole engine.",
      "Your OCR and memory-map add-ons now survive a repair. If PM had to rebuild its document engine — after a torn install, or when it found a newer Python — it deleted the add-ons you'd installed without saying a word: photos silently stopped having their text read, and the memory map quietly dropped to a simpler layout. They're now reinstalled automatically, and if that can't be done you're told rather than left guessing.",
      "Very large text files are now turned away up front instead of after several minutes. A big .txt, .md or .html file was accepted, converted, and only then rejected for being too big to hand back — so you waited, and got an error the whole time was spent earning. The limit for those files now reflects what PM can actually return.",
      "After an update, PM no longer claims its document engine is ready before checking. It could report Ready while still set up for the previous version's requirements, so the first thing you did in that window ran against the wrong setup.",
    ],
  },
  {
    version: "3.19.7-alpha",
    date: "2026-07-16",
    highlights: [
      "A chat reply that fails partway through is no longer saved as if it were the answer. If the model or its provider failed after it had started replying, PM kept the half-finished text, saved it into the conversation and your vault, indexed it, and said nothing — so a broken sentence became something PM could quote back to you later as though it were real. A failure is now reported as a failure. And when a reply is cut short simply because it got long, it's saved with a note saying so rather than pretending it just ended there.",
      "Renaming a chat before you've sent anything now sticks. PM names conversations for you in the background, but a title you chose yourself is meant to win. Renaming a brand-new chat and then sending your first message let the automatic namer overwrite what you'd picked — which is exactly the moment people rename a chat.",
      "Background work now bills the background key, as intended. If you've set a separate key for background jobs, the rolling summaries, chat titles and preference learning were all still charged to your main key — so the split you set up didn't reflect where the spend went, and a background-key-only setup refused to run them at all. Nothing changes if you use a single key.",
      "Setting an empty Microsoft client ID is now refused instead of accepted and then failing later with an unhelpful error.",
    ],
  },
  {
    version: "3.19.6-alpha",
    date: "2026-07-16",
    highlights: [
      "Reminders about your day now use your day. If you're not on UTC, PM worked out whether an event was \"happening today\" or still ahead of you by reading the date off however your calendar provider happened to store it — not by asking what day it is where you are. An evening event could be treated as tomorrow's, so the nudge arrived the morning after it happened. Your timezone now decides, the same way the rest of PM already did.",
      'Subscribed calendar feeds that use lowercase now work. The iCalendar standard says property names can be written in any case, but PM only recognised capitals — so a feed writing "dtstart" instead of "DTSTART" quietly appeared completely empty, and one writing "rrule" lost every repeat of a recurring event. Both now read correctly.',
      'Events from Google and Outlook now sort together properly. Google reports times in the calendar\'s own timezone while Outlook and subscribed feeds report them in UTC, and PM compared them as plain text — so an event could appear in the wrong place in an agenda that mixed the two, and "the next one" could pick the wrong event. All times are now stored the same way. Your calendars will resync once, on their own.',
      "Deleting a milestone now clears its reminders. The milestone went, but any flag it had raised stayed behind — including ones you'd already dismissed, which nothing ever removed.",
    ],
  },
  {
    version: "3.19.5-alpha",
    date: "2026-07-16",
    highlights: [
      "Files from your connected accounts are no longer buried in search results. PM doesn't keep the contents of Drive or OneDrive files on your machine, so it stored a short summary and left a placeholder — \"(body available at the source)\" — where the text would go. Several parts of PM read that placeholder as if it were the file: the step that ranks results by relevance judged every connected file on that one sentence and pushed them all to the bottom, chat answers cited files it couldn't actually read, and the assistant that suggests a project's details saw the same line for every file. They all now read the summary PM does have.",
      "Renaming a note now renames it everywhere. Changing a note's title on the Pinboard and saving it left the old title on the document itself — in search, in citations, and in the file in your vault — because PM decided nothing had changed by looking only at the body. The title counts as a change now, and the note is re-indexed under its new name.",
      "A file that fails to import no longer comes back on its own. When importing a document, photo or spreadsheet failed at the last step, PM had already written the file into your vault and left it there — so the next time you rebuilt the index, the failed import reappeared as a document, unsorted and with no sign anything had gone wrong. The leftover file is now cleaned up.",
      "Under-the-hood tidying: projects created from your connected files now stick properly instead of being forgotten and recreated on every launch, and a connected file that can't be restored during a rebuild is named on screen instead of leaving the progress bar stuck just short of the end.",
    ],
  },
  {
    version: "3.19.4-alpha",
    date: "2026-07-16",
    highlights: [
      "Removing PM's data now clears every vault key it kept, not just the current one. \"Remove PM data\" promises to erase every secret PM has stored on your machine. If you had joined someone else's shared vault and later left it, PM deliberately keeps its key so rejoining is silent — but the erase only ever cleared the vault you were using at the time, so that key stayed on the machine afterwards, for a vault that may still be sitting in a shared folder. Every key PM has cached is now erased, including from vaults you left before this update.",
      "Moving your vault now takes all of it. Two encrypted files — your entity rules and the list of files indexed from cloud accounts — were left behind in the old folder every time the vault moved. Backups always included them, so nothing was ever lost, but making a vault private left readable copies in the folder you were leaving. They now travel with the vault, and deleting a shared vault properly empties the folder instead of leaving those files in it.",
      "Unlocking your vault no longer fails because Windows couldn't remember the key. If PM couldn't save your key for next time — a credential store that's disabled or damaged — it treated that as a failed unlock and refused to open a vault your passphrase had already opened correctly, with no way forward. It now opens as normal and simply mentions you'll be asked for the passphrase again next launch.",
    ],
  },
  {
    version: "3.19.3-alpha",
    date: "2026-07-16",
    highlights: [
      "Your pinboard can no longer be replaced by an empty one. If PM couldn't read your board when you opened the Pinboard — a moment's trouble reading from your data folder was enough — it showed an empty board and then treated that as yours: simply switching to another tab saved the blank one over the real one. PM now says it couldn't open your board, leaves what's saved untouched until it can read it properly, and gives you a Retry.",
      "The Pinboard now tells you when it can't save. It promises your board is \"saved on this device\", but if a save actually failed it said nothing at all and carried on — so you could keep arranging a board that wasn't being kept. It now says so plainly, and your next change tries again by itself.",
    ],
  },
  {
    version: "3.19.2-alpha",
    date: "2026-07-16",
    highlights: [
      "Photos you save into an encrypted vault now survive a passphrase change. When you tick \"save a copy to the vault\" for an image, PM keeps that copy encrypted alongside your notes. Changing your vault's passphrase re-locked the notes but quietly left those images locked to the old one — so the copy you kept, often after deleting the original, could no longer be opened. Photos are now re-locked along with everything else, and making a vault private unlocks them too. If you changed a passphrase before this update, those images can only be recovered from a backup made beforehand — PM will now fall back to showing the original file or the photo's text rather than failing outright.",
      'Saving a copy of an image you\'d already added now sticks. Re-adding a photo with "save a copy to the vault" ticked said it had saved one, but only half the record was written — so the next time you rebuilt the search index, PM forgot the copy existed and left it stranded in your vault. The copy is now recorded properly, and a rebuild repairs any that were stranded this way.',
      "Exporting your vault as plain files now includes your saved photos. The export is there so you're never locked in, but it only ever wrote out your notes — the saved images stayed behind, encrypted. They now come out too, under their real filenames.",
    ],
  },
  {
    version: "3.19.1-alpha",
    date: "2026-07-16",
    highlights: [
      "Fixed a passphrase trap in shared vaults. If you set up or changed a shared vault's passphrase with a space at the start or end, PM quietly dropped those spaces when it locked the vault — but not when it unlocked it — so the passphrase you'd chosen wouldn't open your own vault. Your passphrase is now taken exactly as you type it, spaces and all. Vaults set up before this update still open with the spaces left off: PM now says so if what you're typing starts or ends with a space, and changing the passphrase re-keys the vault to exactly what you type.",
      "A passphrase can no longer start or end with a space. It's almost always a stray one from a copy-and-paste, you can't see it in a password box, and you'd have to type it exactly right forever after — so PM now says so while you're choosing, rather than letting you find out the hard way. Spaces inside are still very much welcome: a passphrase of a few plain words is still the best kind.",
    ],
  },
  {
    version: "3.19.0-alpha",
    date: "2026-07-16",
    highlights: [
      "Rebuilding the search index no longer starts over if it's interrupted. Close PM (or lose power) at 95% through a rebuild and it used to throw all of that away and begin again from nothing — the slowest thing PM does, done twice. Now it picks up exactly where it left off, including the connected files it had already re-read from Google Drive or OneDrive, so nothing gets downloaded or re-read twice.",
      "Search keeps working while a rebuild runs. A rebuild used to clear the whole index first and refill it, so for as long as it took — sometimes hours on a big library — searching and chat found less and less of your library. Now each document is rebuilt in place, so your search stays intact the whole way through. (Switching search language is the one exception: that one still has to clear the index first, because the whole shape of it changes.)",
      "Rebuilding no longer forgets what you'd taught PM. Every rebuild used to quietly cut the link between your filing corrections and the documents they were about, which is the record PM learns your habits from. Those links now survive. Old links to documents in past chats survive too, so citations in your chat history keep working after a rebuild.",
      "PM now stands its ground while it's rebuilding. Adding documents, syncing a connected account, or changing your vault while a rebuild was running could quietly collide with it. Those now wait — and tell you why — instead of racing it.",
    ],
  },
  {
    version: "3.18.2-alpha",
    date: "2026-07-15",
    highlights: [
      "Under-the-hood tidying: PM's own documentation caught up with the last two updates. Nothing in the app changed — the README was still telling you that you could pick any model through OpenRouter, which stopped being true when the model list started filtering out ones that can't answer privately.",
    ],
  },
  {
    version: "3.18.1-alpha",
    date: "2026-07-15",
    highlights: [
      "Rebuilding your documents no longer loses its place when you switch tabs. It never actually stopped — the work carried on the whole time — but the Documents tab forgot it was happening the moment you looked away, came back showing nothing, and left you assuming it had died. Now the progress is yours to leave: switch tabs, go and do something else, and the bar picks up exactly where it is when you return.",
      "It also survives closing the app. If PM is shut mid-rebuild, it starts the rebuild again by itself next time you open it. Being straight with you: a rebuild can't run while the app is closed, and it has no way to pick up mid-file, so this restarts it rather than resuming it. It's still the right thing to do — a rebuild that's been interrupted leaves your search incomplete until it finishes, and previously nothing told you that or fixed it.",
      "Fixed a real one: rebuilding twice at once could destroy the first rebuild's work. Switching away and back reset the button, so a second Rebuild could start while the first was still going — and the second one clears the index out before it begins. PM now refuses the second and tells you the first is still running.",
    ],
  },
  {
    version: "3.18.0-alpha",
    date: "2026-07-15",
    highlights: [
      "The model list in Settings › AI now only offers models PM can actually use. PM sends every request with zero-data-retention enforced, so a provider can't store or train on your prompts — and a model with no provider willing to work that way doesn't quietly become less private, it simply doesn't answer. Those models are now filtered out of the picker rather than left there to fail, which takes the list from around 340 models to around 220. Every one that remains is one your prompts are safe with.",
      "Recommended models is gone. It suggested two models and, in fairness, sometimes suggested one that couldn't answer at all — it ranked on price and capability, and had no way to know whether a provider would agree to zero-data-retention. Rather than teach it, PM now applies that knowledge to the whole list above, where it helps whichever model you pick. The default, Ling-2.6-flash, is doing the job well on its own. Your saved models are untouched.",
      "If you'd set up recommendation exclusions, that list has gone with it. It only ever hid models from those two suggestions and never blocked anything you chose yourself, so nothing you rely on changes.",
      "You can still type a model id by hand, as always — PM isn't locked to the list.",
    ],
  },
  {
    version: "3.17.1-alpha",
    release: true,
    date: "2026-07-15",
    highlights: [
      "PM 3.17.1 rolls up everything since the last release into one update. Here's the tour at a glance — every line below has its full story in the entries that follow.",
      "One thing to do after updating: open the Documents tab and click Rebuild, once. Files you'd connected from Google Drive, OneDrive or a watched folder were only searchable by a short summary of themselves, so search, chat and the filing suggestions in Review could all miss what was deeper inside them. They're now read and indexed in full — and that one Rebuild is what brings the files you already have up to date. Anything offline at the time stays findable by its summary and catches up on its next sync.",
      "The pinboard is now something you can properly work on. Folders are made on purpose with a + Folder button, they're resizable, and they stay until you ungroup them rather than evaporating when they're down to one card. A card only files into a folder if your mouse is over it when you let go — so a big note can finally sit next to a folder without being swallowed — and folders no longer swallow each other. Opening one as an overlay gives you a real board to drag, resize and overlap on, not a list.",
      "The pinboard has undo, and asks before it deletes. Ctrl+Z (⌘Z on a Mac) takes back your last change, Ctrl+Y puts it back, and deleting a note or timeline now tells you what actually goes with it first — with a “Don't ask again” tick, and a switch in Settings › General to bring the asking back. Two things undo deliberately won't take back, because it can't do so honestly: ingesting a note, and linking a timeline to a project.",
      "Your milestones and timelines now show on the calendar. Any dated milestone appears as an all-day marker in its own colour across Month, Week, Day and Agenda — click it to jump to its project — and every dated entry on a pinboard timeline does the same. You can hide either from the Calendars menu. The Milestones panel also sorts now: by deadline, by name, or by hand, from a Manual / Deadline / Name control in its header.",
      "The calendar scrolls, and its timezones moved somewhere you'll find them. Month and Year views now scroll smoothly through past and future instead of jumping a whole month or year, settling neatly on a week or a row of months. Extra timezones are added from the top-left corner of the Day or Week grid, where the hour column meets the dates, and the list reads as Continent / Country / City so you can search by any of them.",
      "PM now starts you on a far cheaper model — Ling-2.6-flash instead of Claude Sonnet 4.6, a few hundred times less per word — which matters most for the work PM does when you're not watching it. Sonnet is still there in Settings › AI. The honest trade: it reads a little less at once, and it's served by a single provider, so if that provider has a bad day chat waits rather than quietly moving elsewhere; adding a second model and turning on auto-switch gives you that safety net back. If you've already picked your own models, nothing changes.",
      "Settings is much quieter. Every section leads with what you came to change and folds its explanation behind a caret underneath — for everyone, at every density. Nothing was deleted, and warnings you'd regret missing stay out in the open.",
    ],
  },
  {
    version: "3.17.0-alpha",
    date: "2026-07-15",
    highlights: [
      "PM now starts you on a far cheaper model. Out of the box, chat and background work both use Ling-2.6-flash instead of Claude Sonnet 4.6 — a few hundred times less per word, which matters most for the work PM does when you're not watching it: naming your chats, summarising them, and proposing where each new document belongs. Sonnet is still there in Settings › AI whenever you want it. If you've already picked your own models, nothing changes — this only sets the starting point for anyone who hasn't.",
      "The honest trade: the new default reads a little less at once than Sonnet did (262,000 tokens against a million — still far more than a normal chat needs), and it's served by a single provider, so if that provider has a bad day chat waits rather than quietly moving elsewhere. Adding a second model in Settings › AI and turning on auto-switch gives you that safety net back.",
      "Settings is much quieter. Every section now leads with what you came to change — the switches, the pickers, the connect buttons — and folds its explanation away behind a caret underneath. Nothing was deleted; it's one click away, and Help mode still explains anything you hover. What deliberately stays out in the open: warnings you'd regret missing (there's no recovering a backup without its passphrase), and the notes telling you why a button is greyed out.",
      "Those explanations used to unfold themselves for the Power density, on the theory that a power user wants more detail. That had it backwards — the person most likely to already know how backups work was the one being handed the essay. They now start folded for everyone, at every density. Your connected accounts — Google, Microsoft, Apple — are unaffected and still sit open where they always did.",
    ],
  },
  {
    version: "3.16.0-alpha",
    date: "2026-07-15",
    highlights: [
      "The pinboard now has undo. Ctrl+Z (⌘Z on a Mac) takes back what you last did — deleting a card, changing a note's colour, moving something, or the last few seconds of typing — and Ctrl+Y, or Ctrl/⌘+Shift+Z, puts it back. Typing is grouped into short bursts, so one undo takes back a few seconds' worth rather than a single letter, and it leaves your cursor where the change was. The trail lasts for as long as you have PM open; it isn't saved between visits.",
      "Deleting a note or timeline now asks first. The pop-up tells you what actually goes with it — dated entries that would leave your calendar, or a document you'd already saved to your vault that would stay behind. There's a “Don't ask again” tick if you'd rather it didn't, and a switch in Settings › General to bring it back. A folder's ✕ doesn't ask, because it only ungroups the folder and spills the cards back onto the board.",
      "Two things undo deliberately won't take back, because it can't do so honestly: ingesting a note (the document is its own copy in your vault, and stays there), and linking a timeline to a project (its entries become that project's real milestones). Linking is the point where the undo trail starts fresh.",
    ],
  },
  {
    version: "3.15.0-alpha",
    date: "2026-07-15",
    highlights: [
      "Opening a folder as an overlay now gives you a pinboard, not a list. It fills 80% of the board, and the notes and timelines inside keep the shape and size you gave them — you can drag them around, resize them and overlap them in there exactly as you do outside. Cards are laid out clear of each other when they go in, so nothing hides behind anything else. Opening a folder “in place” is unchanged, if you prefer the compact card view.",
      "The board is now a fixed size — your screen — instead of quietly resizing with the window. It used to be as wide as the window and grew downward to wherever your lowest note sat, so the same board changed shape depending on how you'd sized PM. Now it's one canvas that simply scrolls if the window is smaller than it, and a board you made on a bigger monitor still tidies itself in to fit.",
      "Fixed: help mode couldn't be read inside any pop-up window — its labels were painted behind them. They now sit on top.",
      "Fixed: What's New asked to be 80% of the window's height and was silently getting 85%.",
      "Under-the-hood tidying: the board's drag-and-drop, its grid, and its cards are now one shared implementation used by both the main board and the one inside a folder, so the two can't drift apart. Dragging with a folder open is also markedly less work for your machine.",
    ],
  },
  {
    version: "3.14.0-alpha",
    date: "2026-07-15",
    highlights: [
      "Pinboard folders are now something you make on purpose. There's a + Folder button beside + Note and + Timeline that drops an empty folder on the board, ready to tidy things into. Stacking one card exactly on top of another still folds the two together, as before.",
      "Filing a card into a folder now follows your mouse, not the card. Until now, dragging a note so that any part of it merely touched a folder was enough for the folder to swallow it — which made big notes almost impossible to park near a folder. Now a card only goes in if your pointer is over the folder when you let go, and the folder lights up to show it's about to catch it. Anywhere else, the card just lands where you put it and overlaps, the way notes and timelines already do.",
      "Folders no longer swallow each other. Dragging a folder onto another one used to tip its notes into it and destroy the folder you were holding. Now they simply stack, each keeping its own cards — and a folder dropped squarely on another shuffles over a cell so you can still get at the one underneath.",
      "A folder now stays until you ungroup it. It used to vanish the moment it was down to one card, popping that card back onto the board. That made a folder feel like a temporary side-effect rather than a thing you own — and an empty one you'd just made couldn't survive at all. Its ✕ still ungroups it and spills the cards back out, which remains the way to get rid of one.",
      "Fixed: opening a folder in place could squeeze a note's card so narrowly that its delete button was pushed out of sight and its title had no room left. The panel now lays its cards out in two columns whatever the size of your window (it was quietly using three on a wide screen), and each card's title is guaranteed a bit of space of its own.",
    ],
  },
  {
    version: "3.13.0-alpha",
    date: "2026-07-15",
    highlights: [
      "You can now sort a project's milestones. The Milestones panel has a small Manual / Deadline / Name control in its header: sort by deadline (click again for latest-first) or by name, or keep arranging them by hand with the up/down arrows as before. Undated milestones settle at the bottom.",
      "Your milestones now show on the calendar. Any dated milestone that isn't already on one of your calendars appears as an all-day marker in its own colour across Month, Week, Day and Agenda (and the Terminal look) — one central place to see everything. Click one to jump straight to its project. Completed ones show a ✓, and you can hide them from the Calendars menu. (A milestone you'd tied to a real calendar event keeps showing as that event, in its calendar's colour.)",
      "Your pinboard timelines show on the calendar too. Every dated entry on a freeform timeline appears as an all-day marker in its own colour — click one to jump back to the Pinboard. Each freeform timeline has a “Show on calendar” tick if you'd rather keep one off, and you can hide the lot from the Calendars menu. Link a timeline to a project and its entries become that project's milestones, shown in the milestone colour instead.",
      "The little picker for tying a milestone to a calendar event has gone. Now that milestones show on the calendar by themselves, it was more fiddle than help — and each milestone row has more room for its name, with the up/down arrows moved down beside the date. Milestones you had already linked keep their synced date and can still be unlinked.",
      "Linking a pinboard timeline to a project now keeps the entries you typed: they're added to that project's milestones instead of disappearing behind the linked view. Unlinking still leaves them safely in the project. The timeline's + Milestone button now shares a row with the project box, so the card wastes less space.",
      "Ingesting a note now keeps the title you gave it — that title becomes the document's name in Review, with the note's text as its contents. Untitled notes still take their first line, as before.",
      "Fixed: in Week and Day view, the dates along the top could sit slightly out of step with the columns beneath them whenever the grid showed a scrollbar. The header now reserves exactly the same space, so they line up.",
      "Fixed: if your OpenRouter key was ever saved blank, PM would try to use it anyway and the AI would fail with a confusing 401 “Missing Authentication header”. A blank key now counts as no key at all — you'll get the plain “No OpenRouter API key set” prompt instead, and a blank background key quietly falls back to your main one.",
    ],
  },
  {
    version: "3.12.1-alpha",
    date: "2026-07-14",
    highlights: [
      "Files you connect from Google Drive, OneDrive, or a watched folder are now searched on their full contents. Until now, after PM rebuilt its search index these files were only findable by a short (~500-character) summary — so search and chat could miss things deeper in a document, and the filing suggestions in Review only saw the title. They're now indexed in full.",
      "Because of that fix: after updating, open the Documents tab and click Rebuild once. It now also re-reads your connected files from their source, one at a time, so a single Rebuild brings everything fully up to date — you'll see each file processed. Anything temporarily offline stays findable by its summary and is caught up automatically the next time it syncs.",
      "HTML files from Google Drive are now read as clean text — the page's code (scripts, styles, and head) is stripped before indexing, exactly like files in a watched folder — so only the real content is indexed and summarised.",
    ],
  },
  {
    version: "3.12.0-alpha",
    date: "2026-07-14",
    highlights: [
      "The calendar's Month and Year views now scroll. Drag up and down to move smoothly through past and future — no more jumping a whole month or year at a time.",
      "Month view flows as one continuous run of weeks: let go and it settles neatly on a week, never mid-week. Each new month names itself inline and alternates a faint shade so you can see where one ends and the next begins; the header keeps up with whatever's on screen.",
      "Year view scrolls the same way through its little months, settling on a row of months rather than snapping a whole year. Today, the arrows, and the mini-calendar all glide you to the right spot.",
    ],
  },
  {
    version: "3.11.0-alpha",
    date: "2026-07-14",
    highlights: [
      "Adding an extra timezone to the calendar is easier to find. The separate “Zones” button is gone — instead, look to the top-left corner of the Day or Week grid, where the hour column meets the dates: a small “＋ Add” lives there. Add a zone and it shrinks to a “＋” to save space, and you can remove any extra timezone by hovering its column heading and clicking the ✕ that appears.",
      "The timezone list is far easier to search. Every zone now reads as Continent / Country / City with its code or UTC offset alongside — so you can find one by continent, country, city, or code. Can't find your exact city? Search your country and pick the nearest one listed.",
    ],
  },
  {
    version: "3.10.0-alpha",
    date: "2026-07-14",
    highlights: [
      "More pinboard polish, all from living with it. Folders are now resizable like notes and timelines — drag a folder's corner to make it as big or small as you like.",
      "The pinboard is now a fixed-width canvas the size of your window: adding notes fills left-to-right and wraps to a new row instead of stretching the board off the edge and taking the buttons with it. The board only ever grows downward (and scrolls), and a note you add scrolls into view. Notes from before that had drifted off to the right tidy themselves back on-screen.",
      "Note titles can be much longer now, stretching nearly to the Ingest button. The colour dots stay out of the way until you're actually editing a note — and they name themselves by colour on hover: Sage, Coral, Amber, Stone, Teal.",
      "Notes keep their place when you switch to another app and back — a note you're writing stays open for editing until you click somewhere else on the board, rather than snapping shut the moment PM loses focus.",
      "Timelines can switch between a stacked list and a horizontal track from a little toggle in their top bar, and the date column is a touch tidier.",
      "Checklists start flush against the left edge instead of looking pre-indented. Press Tab to nest an item under the one above (new checkboxes keep that level automatically), and Shift+Tab or Backspace to pull it back out.",
    ],
  },
  {
    version: "3.9.1-alpha",
    release: true,
    date: "2026-07-13",
    highlights: [
      "PM 3.9.1 rolls up everything since the last release into one update. Here's the tour at a glance — every line below has its full story in the entries that follow.",
      "See other timezones on the calendar, and set your own hours. In Day and Week view you can add up to two more timezones down the left beside your local time, frame the view to your own Work or Day hours from the ▾ on those buttons, and events now show their start and end time (e.g. 09:30–10:45) instead of just the start.",
      "A tidier pinboard. Notes and timelines can have a title, the Ingest button moved up into the top bar to give notes their full height back, and dropping a note exactly on top of another the same size folds them into a neat folder tile you can name, open, and drag cards back out of. Every colour also names itself on hover now.",
      "Your vault recovers honestly if Windows ever blocks its folder. Instead of a confusing wall of “the vault is locked” messages, PM now says exactly what's wrong and offers a one-click “Repair access”. Sharing a vault across Windows accounts is safe by construction — PM checks the destination, locks it down and confirms it still works before committing — and you can properly delete a shared vault, with everyone who joined told plainly and moved back to their own. Your data was never at risk; now PM says so.",
      "Windows updates no longer fail silently. If Smart App Control is blocking PM's update installer, PM now spots it and tells you the one thing that fixes it, instead of quietly reopening on the old version every launch.",
      "A cleaner exit. Finishing “Remove PM data” now actually closes the app and a full uninstall leaves no leftover “PM” folder behind, plus a clear reminder before you erase a vault and database that can't be recovered.",
    ],
  },
  {
    version: "3.9.0-alpha",
    date: "2026-07-13",
    highlights: [
      "See other timezones on the calendar. In Day and Week view, add up to two more timezones from the “Zones” button and they show as extra columns down the left beside your local time — handy for a call with another city. Remove them the same way.",
      "Set your own Work and Day hours. The ▾ on the Work and Day buttons (Day/Week view) opens a little editor for the start and end of each. Work now frames 08:30–17:30 by default, so a 9-to-5 day's first and last events sit comfortably inside the view; Day frames your local sunrise-to-sunset, rounded to the hour so it only shifts with the seasons. Change either to whatever suits you, or reset to the default.",
      "Calendar events now show their start and end time (e.g. 09:30–10:45), not just the start.",
      "Under the hood: your timezone and location now come from one shared place, so the calendar, the sunrise/sunset day framing, and the automatic light/dark switch all stay in step.",
    ],
  },
  {
    version: "3.8.0-alpha",
    date: "2026-07-13",
    highlights: [
      "Pinboard notes and timelines can now have a title — click the label at the top of a card (it starts as “Note” or “Timeline”) and type your own. The Ingest button moved up into that top bar too, so notes get their full height back for what you're actually writing.",
      "Stack it to file it: drop a note exactly on top of another the same size and the two fold into a tidy folder tile that shows how many cards are inside. Open it to read and edit them in place (or as a pop-out overlay), give the folder its own name, drag a card back out, and the folder quietly dissolves once it's down to one. Timelines can go in folders too, and a folder's ✕ just spills its cards back onto the board — it never deletes them.",
      "Every colour now tells you its name on hover — the note tint dots on the pinboard and every theme swatch in Settings, not just the monochrome one.",
      "While you're editing a note, hovering the formatting buttons (bold, lists, checklist…) reliably shows what each one does and its keyboard shortcut again.",
    ],
  },
  {
    version: "3.7.2-alpha",
    date: "2026-07-13",
    highlights: [
      "You can now delete a shared vault properly. From Settings → Vault, “Delete shared vault…” removes it from the shared folder for everyone who uses it — with a clear warning first — and switches your account back to a vault of its own. The accounts you shared with are told, plainly, that the vault was deleted the next time they open PM, and are moved back to their own vault instead of hitting a confusing error.",
      "“Remove PM data” from an account that joined someone else's shared vault now only clears your own copy of things and leaves the shared vault untouched for everyone else — it can no longer wipe a shared vault out from under the other accounts.",
    ],
  },
  {
    version: "3.7.1-alpha",
    date: "2026-07-13",
    highlights: [
      "Sharing a vault is now safe by construction: PM checks the destination folder can actually hold a shared vault before it moves anything, locks the folder down and confirms it can still open the vault there, and only then commits the move. If any of that fails, the move is cancelled and your vault stays exactly where it was — the earlier failure that could leave a vault stranded in a folder Windows then blocked can no longer happen.",
      "PM won't let you put a shared vault somewhere it can't work — a network drive, or a USB stick formatted as FAT32/exFAT (which can't store per-account permissions) — and says so up front instead of failing partway through.",
      "Making a shared vault private again now moves it back to your own private folder first, then decrypts — so your notes are never briefly written in readable form inside a folder other accounts can reach.",
      "When you add an account to a shared vault, PM now double-checks the permission actually took effect, so a silent “looked fine but didn't work” can't slip through.",
    ],
  },
  {
    version: "3.7.0-alpha",
    date: "2026-07-13",
    highlights: [
      "If Windows ever blocks PM from its own vault folder, PM now says exactly that — and offers a one-click “Repair access” that fixes the folder's permissions and opens your vault again. Before, a blocked folder showed up as a confusing wall of “the vault is locked” messages, a restart landed on a dead-end error screen, and rejoining looked like your passphrase had stopped working. Your data was never gone — now PM says so, in plain words, on every screen it could happen.",
      "Every vault problem now tells you what's actually wrong and what to do about it: a folder PM can't reach points to Repair access, a folder that's gone says so, a wrong passphrase says it's the passphrase (and only when it really is), and a damaged vault file is called damaged — four different problems that used to look identical.",
      "“Use a vault on this account instead” now explains what will happen before it does anything — whether you'll get back the vault that was set aside when you joined, or start with an empty one — and PM remembers the shared vault you left, so Settings can offer “Rejoin” with one click later. Nothing in the shared folder is ever deleted.",
      "Two protections against losing a shared vault by accident: “Remove PM data” on an account that's joined a shared vault now cleans up only that account's own data and leaves the shared vault untouched for everyone else, and the “start fresh” recovery can no longer delete a vault that's merely blocked by permissions.",
    ],
  },
  {
    version: "3.6.4-alpha",
    date: "2026-07-13",
    highlights: [
      "Finishing “Remove PM data” now actually closes the app. The “Close PM” and “Finish uninstall” buttons at the end of the removal flow used to do nothing when clicked, leaving PM open — they now quit it properly. And a full uninstall on Windows no longer strands a leftover “PM” folder: the bundled document-engine files it used to leave behind are now cleaned up with everything else.",
      "One more safeguard before you erase your data — when your vault and database are part of what you're removing, the confirmation screen now reminds you, in plain sight, that they can't be recovered, so you can back them up first if you haven't already.",
    ],
  },
  {
    version: "3.6.3-alpha",
    date: "2026-07-13",
    highlights: [
      "Windows updates no longer fail silently. If Windows Smart App Control is switched on, it blocks PM's update installer with no visible error — clicking “Restart now” would close PM and quietly reopen on the old version. Now PM checks for that first and, when it's the cause, tells you plainly and points to the one thing that fixes it (turning Smart App Control off), instead of trying the same broken update again and again on every launch.",
    ],
  },
  {
    version: "3.6.2-alpha",
    release: true,
    date: "2026-07-13",
    highlights: [
      "PM 3.6.2 rolls up everything since the last release into one update. Here's the tour at a glance — every line below has its full story in the entries that follow.",
      "Share your vault across Windows accounts on this PC — one guided flow moves the vault somewhere every account can reach, you pick who may open it from a list, and the other account joins from a single screen by typing the passphrase. Whatever was already on that account is kept safely aside, never deleted.",
      "PM now runs on Linux (x86_64) — an auto-updating AppImage (the recommended install) and an rpm for Fedora-family systems, both self-contained and built by the same signed pipeline as Windows and macOS.",
      "Index a folder but skip parts of it — when you choose what to index from a Google Drive, OneDrive, or a folder on this computer, uncheck any subfolder to leave it (and everything inside) out. Handy for a big archive or a noisy downloads folder.",
      "Mark a calendar as “Quiet” — it still shows on your Calendar tab, but its events stay out of everything PM brings to your attention: the daily briefing, “due soon” reminders, and chat.",
      "Tidy up after filing — change a document's project or importance once it's already filed, straight from the Documents tab, and clear a review one document at a time instead of committing the whole list.",
      "Sturdier and safer — “Remove PM data” can no longer leave you stuck on a vault that won't open, the “couldn't open your vault” screen now has a way forward, and finding the Proton Drive CLI for encrypted backups is far more reliable. Plus under-the-hood work: Linux keychain support, safer release tooling, and refreshed build dependencies.",
    ],
  },
  {
    version: "3.6.1-alpha",
    date: "2026-07-13",
    highlights: [
      "Sharing your vault with another Windows account on this PC actually works now — and it's one guided flow. Before, “make shareable”, “move vault”, and “link account” were three separate steps that quietly left the vault somewhere other accounts could never reach. Now “Share with other accounts…” walks you through it: choose a passphrase, PM moves the vault to a spot every account can reach, and you pick who may open it from a list of this PC's accounts (no more hunting for SIDs — though you still can).",
      "Joining is now one screen. The first time PM opens on the other account, it spots the shared vault and offers it by name — enter the passphrase and everything shared is there: documents, chats, projects, calendars. Anything already on that account is kept safely aside, never deleted. There's also “Open an existing shared vault…” in Settings for joining by folder.",
      "After joining, PM explains what stays personal: your own AI key and your own cloud sign-ins (they never travel between Windows accounts). A note on the Connectors tab shows exactly what to reconnect, and everything already indexed stays findable meanwhile.",
      "Safety nets all around: PM refuses to move a vault onto a different vault (no more silent overwrites), linked accounts survive later moves, a shared vault that stops answering gets a clear explanation with a one-click way back to a vault of your own, and a passphrase changed on the other account simply asks you for the new one instead of acting broken.",
      "The changelog no longer pops up on a brand-new install pretending something changed — it now only appears after a real update.",
    ],
  },
  {
    version: "3.6.0-alpha",
    date: "2026-07-13",
    highlights: [
      "You can now index a folder but skip parts of it. When you choose which folders to index from a Google Drive, OneDrive, or a folder on this computer, the folder tree lets you uncheck any subfolder to leave it (and everything inside it) out — handy for skipping a big archive or a noisy downloads folder while indexing the rest.",
      "Your choices take effect on that connection’s next sync. Anything already indexed inside a subfolder you’ve just excluded is removed from search then — but the files themselves are never touched on disk or in the cloud.",
    ],
  },
  {
    version: "3.5.0-alpha",
    date: "2026-07-13",
    highlights: [
      "You can now change a document’s project and importance after it’s been filed, straight from the Documents tab. Filed something in the wrong place, or want to raise its importance? Click “Edit” on its row and adjust — nothing is re-processed.",
      "In Review, you can now approve documents one at a time: each row has its own “Approve” button, so you can clear the ones you’re sure about without committing the whole list.",
      "Fixed a small annoyance in Review — choosing an importance no longer re-shuffles the list and scrolls you away from the item you were working on. It stays put.",
      "When you preview how an indexed-only file (from a connected Drive/OneDrive/folder) was split into chunks, PM now shows the highlights correctly, or — if its saved breakdown is out of date — tells you plainly and offers a one-click “Re-index this item” to rebuild it, instead of a dead-end message.",
    ],
  },
  {
    version: "3.4.0-alpha",
    date: "2026-07-13",
    highlights: [
      "New: mark a calendar as “Quiet”. A quiet calendar still shows on your Calendar tab, but its events are kept out of everything PM brings to your attention — the daily briefing, “due soon” reminders, and chat. Ideal for a calendar you want to see but not be nudged about, like a recurring payments tracker. Toggle “Quiet” next to each calendar under Settings → Connectors.",
    ],
  },
  {
    version: "3.3.1-alpha",
    date: "2026-07-13",
    highlights: [
      "Fixed the “Get the Proton Drive CLI” button, which had started leading to a missing page — it now opens Proton’s current guide.",
      "PM finds the Proton Drive CLI more reliably now. It also looks where the download usually lands (like your Downloads folder); if you keep it somewhere else you can point PM straight at it with “Locate manually…”; and a “Check again” button — plus an automatic re-check when you return to the window — picks it up right after you install it, with no restart.",
    ],
  },
  {
    version: "3.3.0-alpha",
    date: "2026-07-13",
    highlights: [
      "Fixed an important issue with “Remove PM data”: if the removal was interrupted — for example by a force-quit, or antivirus briefly locking the file — PM could end up unable to open your vault, stuck on the “couldn’t open your vault” screen with no way forward. Removal now works in a safe order and, if the vault file can’t be deleted, stops without changing anything so you can simply try again.",
      "Added a “Start fresh” option on the “couldn’t open your vault” screen. If the vault ever genuinely can’t be opened, you can now safely reset and set PM up again from there — you’re never stuck. (Your saved keys and sign-ins are kept.)",
      "Choosing to remove everything now uninstalls PM completely: after clearing your data it hands off to the Windows uninstaller and leaves nothing of PM behind on your computer.",
      "Clearer wording on the “Remove PM data” screen about what each choice deletes — your connected accounts and model preferences are stored in the database, so they’re removed with “Vault & database”, while “Saved keys & sign-ins” clears the keys and tokens themselves.",
    ],
  },
  {
    version: "3.2.1-alpha",
    date: "2026-07-12",
    highlights: [
      "Under-the-hood tidying: refreshed several of the development tools PM is built with to their latest maintenance releases. No change to how you use PM.",
    ],
  },
  {
    version: "3.2.0-alpha",
    date: "2026-07-10",
    highlights: [
      "PM now ships for Linux (x86_64): an auto-updating AppImage — the recommended install — and an rpm for Fedora-family systems. Both carry everything PM needs, including its own Python, so the document engine works out of the box with nothing to install.",
      "Linux releases are built, checked, and published by the same pipeline as Windows and macOS, and the AppImage auto-updates exactly like the Windows app.",
      'New in the README: a Linux install guide and a step-by-step "moving between computers" guide — your encrypted backup is the whole migration, on any OS pair.',
      "For the technically curious: the AppImage teaches the document engine to survive relaunches (AppImages mount at a random path each launch, so PM parks its bundled Python in your data folder), and a new no-secrets dry-run workflow proves the whole Linux packaging lane on every packaging change.",
    ],
  },
  {
    version: "3.1.0-alpha",
    date: "2026-07-10",
    highlights: [
      "Groundwork for Linux (first of a short series): on Linux, PM now keeps its secrets in your desktop's real keychain (KWallet or GNOME Keyring), so the key that protects your store survives a reboot — the load-bearing first step toward a supported Linux app. Nothing changes on Windows or macOS.",
      "The app-lock switch on Linux now says honestly that it isn't available yet, instead of offering a lock that any keypress would satisfy. A real Linux lock is on the roadmap.",
      "Fixed: restoring a backup made with the app lock turned on, onto a computer that can't run the verification (a Mac today, a Linux machine tomorrow), no longer shows a lock screen that can never pass — PM opens normally, tells you the lock is inactive on this device, and the setting re-arms on a machine that can verify.",
      "Shared vaults on Linux now get an OS-level folder lockdown (owner-only access, linked accounts re-admitted) — the same defence-in-depth Windows applies with file ACLs.",
    ],
  },
  {
    version: "3.0.3-alpha",
    date: "2026-07-10",
    highlights: [
      "Under-the-hood: our release tooling now double-checks that the notes shown on a version's download page match the version actually being shipped, so a release can't go out with the wrong notes attached. Nothing changes for you day to day.",
    ],
  },
  {
    version: "3.0.2-alpha",
    release: true,
    date: "2026-07-10",
    highlights: [
      "Fixed: a brand-new vault could freeze PM moments after its very first launch — the window went grey and “Not Responding” and never recovered. First starts now boot cleanly.",
      "Under-the-hood tidying across the whole app: a codebase-wide cleanup pass that removes duplicated code and trims wasted work. A few things get snappier along the way — dragging notes on the pinboard, panning the knowledge map, importing documents, and calendar sync with several accounts.",
    ],
  },
  {
    version: "3.0.1-alpha",
    date: "2026-07-08",
    highlights: [
      "Under-the-hood tidying: development builds no longer leave behind an ever-growing compiler cache that could quietly swallow hundreds of gigabytes of disk. Nothing changes for you day to day.",
    ],
  },
  {
    version: "3.0.0-alpha",
    release: true,
    date: "2026-07-08",
    highlights: [
      "PM 3.0.0 rolls up everything since the last public release into one update. Here's the tour at a glance — every line below has its full story in the entries that follow.",
      "Connect your clouds — Google Drive and OneDrive are indexed in place: what's in them turns up in your search, and nothing is copied out of your drive.",
      "Watch folders on this computer — point PM at a folder and it keeps itself current as you work; an edit is searchable within seconds.",
      "A real calendar — Google (several accounts), Outlook and iCal subscriptions together, in Month, Week, Day, Year and Agenda views.",
      "Chats are part of your memory — past conversations become searchable, answers cite the exact turn they drew from, chats name themselves, and each project keeps its own.",
      "A briefing that tracks instead of narrates — deadlines, today's events and prep-ahead nudges are real items you can mark done, or just tell it “the deck is done” in plain words.",
      "Projects with real milestones — several dated deadlines per project, priorities you set (or PM infers for projects other work waits on), and a sortable Focus view.",
      "Encrypted backups — your whole vault in one passphrase-protected file, on demand or on a schedule, to your own Proton Drive or Google Drive.",
      "A map of your knowledge — your documents arranged by meaning on a fast, navigable canvas.",
      "A built-in reader — click any document, or any source a chat answer cites, and read it right there; cloud items fetch their full text on demand.",
      "Photos and spreadsheets, done properly — screenshots read with on-device text recognition; spreadsheets indexed row by row, with one-click full import for Google Sheets.",
      "A Pinboard that keeps up — notes with real formatting, timelines linked to real projects, and one-click ingest of a note into your vault.",
      "Teach PM your world — merge and rename project names for good, and keep structured preferences that PM applies exactly where they fit.",
      "Sharper search — structure-aware chunking, a re-ranking second pass, multilingual vaults, and proper Chinese/Japanese/Korean keyword search.",
      "Quality of life — a calm new monochrome look with a sun-following Auto mode, a Storage tab to reclaim space, one-click Mac setup, a tidy uninstall, and a read-only Developer mode for the curious.",
    ],
  },
  {
    version: "2.92.57-alpha",
    date: "2026-07-07",
    highlights: [
      "Faster first-time indexing of large connected folders. Building the index for a big Google Drive, OneDrive, or local folder no longer slows down the further it gets — the encrypted index is now saved in batches instead of being fully rewritten after every single file, so a folder with thousands of items indexes in a fraction of the time. Nothing changes about what gets indexed or how it's sorted, and an interrupted sync still picks up cleanly where it left off.",
    ],
  },
  {
    version: "2.92.56-alpha",
    date: "2026-07-07",
    highlights: [
      "Your calendar agenda now follows your own day, not UTC's. An all-day event stays on the agenda for exactly the day you see it — no vanishing before midnight or lingering into tomorrow when your zone is far from UTC — and the same day-boundary now applies to the schedule your chat and project matching read. On the Focus view, an event that already finished today stays listed but greyed until your local midnight, so today still looks like a full day. No setup — it uses the time zone you've already set.",
    ],
  },
  {
    version: "2.92.55-alpha",
    date: "2026-07-07",
    highlights: [
      "A batch of reliability fixes. PM no longer churns files inside ignored folders (like node_modules or .git) in and out of a tracked local folder; a folder too large to list in one pass keeps its extra files instead of dropping them; and a folder whose sync hits an error now says so, instead of showing 'all good'. Preferences you state in a long chat are no longer skipped when there's a backlog. A spreadsheet column name containing a '|' no longer breaks its table, and 'you're prepared' no longer shows on the wrong occurrence of a repeating event.",
    ],
  },
  {
    version: "2.92.54-alpha",
    date: "2026-07-07",
    highlights: [
      "Deadlines and calendar dates respect your time zone at the day boundary. A calendar-linked milestone or a project's 'Due soon' countdown near midnight is now counted on the date you actually see it, in your own zone — so nothing reads a day early or late when your zone is far from UTC. No change to how anything looks.",
    ],
  },
  {
    version: "2.92.53-alpha",
    date: "2026-07-07",
    highlights: [
      "Glitch-free fast switching. Rapidly switching between conversations or documents no longer lets a slow background load land on the wrong one — the context meter, the chunk overlay, and the document chunk panel now ignore results for a view you've already moved on from. And after switching the chat to a larger-context model, the meter refreshes once the switch has actually taken effect. No change to how anything works.",
    ],
  },
  {
    version: "2.92.52-alpha",
    date: "2026-07-07",
    highlights: [
      "More robust vault changes and multi-computer coordination. When PM changes how your vault is protected — setting or changing a passphrase, or moving it to a shared folder — it now commits and cleans up in a safer order, so an interrupted change leaves no stray leftovers and can't briefly keep using the previous key. And when you run PM on more than one computer against a single shared vault, a crashed instance can no longer leave the other one waiting to take over. No change to how anything looks or works.",
    ],
  },
  {
    version: "2.92.51-alpha",
    date: "2026-07-07",
    highlights: [
      "Sturdier handling of calendar feeds and pinboard notes. PM is now more resilient to unusual or malformed calendar (.ics) data, follows calendar-feed redirects more carefully, and is stricter about the internal filename it derives for a note. Normal feeds and notes are unaffected — no change to how anything looks or works.",
    ],
  },
  {
    version: "2.92.50-alpha",
    date: "2026-07-07",
    highlights: [
      "An extra safeguard so updates never touch your saved work. PM now verifies automatically, on every build, that a database upgrade step can't delete or overwrite the projects, priorities and notes you've added — a belt-and-braces backstop behind the existing rule that updates only ever add to your data. No change to how anything looks or works.",
    ],
  },
  {
    version: "2.92.49-alpha",
    date: "2026-07-07",
    highlights: [
      "Fixed a rare lockout when setting up a shareable vault. If your vault passphrase began or ended with a space, PM could store it one way but check it another and then refuse to unlock. PM now uses your passphrase exactly as typed — spaces and all — on every screen. Existing vaults are unaffected.",
    ],
  },
  {
    version: "2.92.48-alpha",
    date: "2026-07-07",
    highlights: [
      'Clearer about what leaves your device. The README and security policy now spell out the full, short list of network traffic PM makes — model calls, an update check, a one-time first-run download of its local models, and any calendar or encrypted backups you set up — so "local-first" is stated precisely. Internal test data also had a few sample names tidied up. No change to how anything works.',
    ],
  },
  {
    version: "2.92.47-alpha",
    date: "2026-07-07",
    highlights: [
      "Under-the-hood tidying. Each PM release now publishes a checksum file so you can verify an installer you downloaded by hand; the optional add-on components (photo text recognition, the knowledge-map reducer) are now covered by the same dependency security scanning as the core, and that scanning flags a wider range of advisories. No change to how anything works.",
    ],
  },
  {
    version: "2.92.46-alpha",
    date: "2026-07-07",
    highlights: [
      "Spreadsheet reading is hardened against booby-trapped files. When PM reads an Excel (.xlsx) spreadsheet, it now guards against one crafted to balloon memory or abuse its internal XML, on top of the existing size limits. Normal spreadsheets are unaffected.",
      "PM no longer reads legacy .xls spreadsheets. The decades-old Excel format needs a parser with a weak safety record on untrusted files, so PM now handles only modern .xlsx and .csv. Your .xls files stay on disk untouched — re-save one as .xlsx and PM will index it. Any .xls files already indexed drop out of search on the next sync.",
    ],
  },
  {
    version: "2.92.45-alpha",
    date: "2026-07-07",
    highlights: [
      "Calendar feed subscriptions are hardened against a network redirection trick. When PM fetches a calendar (.ics) feed, it now pins the exact address it already verified is safe — so a malicious feed can't quietly point PM at an address inside your own network between when you add the feed and when it's fetched. Normal feeds are unaffected.",
    ],
  },
  {
    version: "2.92.44-alpha",
    date: "2026-07-07",
    highlights: [
      "Disconnecting a Google account now fully cuts off PM's access. When you disconnect a Google Drive or Calendar account, PM tells Google to sever the connection — so PM drops off your account's connected-apps list right away, instead of lingering until the sign-in expires on its own. Microsoft accounts (which can't be revoked from inside an app) now show a one-click link to finish removing access yourself.",
      "Under-the-hood hardening. A release build of PM is stricter about where it looks for its bundled helper program, and it no longer follows a shortcut (symlink) that points outside a folder you're indexing — so indexing a folder can't quietly pull in files from elsewhere on your disk. No change to how anything looks or works.",
    ],
  },
  {
    version: "2.92.43-alpha",
    date: "2026-07-07",
    highlights: [
      "Stronger protection for the passphrases that lock your data. When you set a passphrase for a shareable vault or an encrypted backup, PM now checks it's genuinely hard to guess — with a live strength meter as you type — and won't accept a weak one. Your existing passphrases still open everything exactly as before; this only applies when you set or change one.",
      "A shared vault can't be quietly downgraded. If a vault you share between computers has its 'keep notes encrypted at rest' setting changed behind your back, PM now notices when it opens the vault, switches encryption back on, and tells you — so your notes never silently start saving as plain text.",
    ],
  },
  {
    version: "2.92.42-alpha",
    date: "2026-07-07",
    highlights: [
      "Your documents and calendar entries can't dress themselves up as PM's own formatting. When PM cites sources in an answer, or lists your agenda, the text inside a document or a calendar event is now kept clearly separate from PM's own citation numbers and agenda lines — so nothing you've saved can be made to look like it came from PM itself.",
    ],
  },
  {
    version: "2.92.41-alpha",
    date: "2026-07-06",
    highlights: [
      "When editing a milestone doesn't go through, PM now tells you. Adding, renaming, moving, ticking off, or removing a milestone — on a project's Focus view or a pinboard timeline — used to fail silently if something went wrong behind the scenes, looking like it worked until the next refresh quietly undid it. Now you see the error instead.",
      "Clicking 'Use it' twice on an AI-suggested deadline no longer files it twice. The button now settles while the milestone is being added, and a deadline that's already on the project is skipped.",
      "Under-the-hood tidying. Added tests around the vault hand-off between two computers sharing one vault, the sidecar lookup, and the 'Remove all data' keychain sweep — pinning behaviour that had none. No change to how anything works.",
    ],
  },
  {
    version: "2.92.40-alpha",
    date: "2026-07-06",
    highlights: [
      "Your chats show their real name everywhere. When PM titles a conversation for you — or you rename one — that title now also updates in your Documents list and in the citations that point back to a chat, instead of keeping the short snippet it started life with.",
      "'Remove all data' is more thorough. If PM was interrupted partway through moving your vault to a new folder, a wipe now also clears the temporary backup it left behind, so removing everything never leaves a readable copy of your data on disk.",
      "Under-the-hood tidying. Renaming or merging a person or project no longer counts as fresh activity on every document it touches. No change to how anything looks.",
    ],
  },
  {
    version: "2.92.39-alpha",
    date: "2026-07-06",
    highlights: [
      "The knowledge map keeps up with your edits. When you change what's inside a document, PM now re-places it on the map to match its new meaning — previously a small edit that didn't change the document's length could leave it sitting in its old spot until something else on the map shifted.",
      "Under-the-hood tidying. PM now reuses a single connection when talking to your chat models instead of opening a fresh one for every request, and the progress bars for the optional downloads (the knowledge-map reducer, photo text recognition, and the Mac Python setup) all run through one shared piece of plumbing. No change to how any of it looks or works.",
    ],
  },
  {
    version: "2.92.38-alpha",
    date: "2026-07-06",
    highlights: [
      "Steadier cloud syncing and rebuilds. If a sync can't fully reach one of your connected accounts, PM now marks that account as needing another try instead of quietly treating the partial pass as complete — so nothing gets skipped over. And if rebuilding the search index can't restore your cloud-indexed items, it now says so plainly rather than finishing as if all was well.",
      "A little more careful with backup passphrases. The passphrase you type for an encrypted backup or restore is now wiped from memory as soon as PM is done with it, matching how PM already handles your vault passphrase.",
    ],
  },
  {
    version: "2.92.37-alpha",
    date: "2026-07-06",
    highlights: [
      "Behind the scenes: refreshed the project's README so its feature tour reflects what PM has grown into — indexing your cloud and local files without copying them, chats becoming searchable, the daily attention layer, the knowledge map, and encrypted restore-anywhere backups among them. No change to how you use PM.",
    ],
  },
  {
    version: "2.92.36-alpha",
    date: "2026-07-06",
    highlights: [
      "OneDrive files now remember their folder. When PM indexes a file from OneDrive, it notes which folder the file sits in — the same detail it already keeps for Google Drive files — so that folder is there as context when you look at where a document came from.",
    ],
  },
  {
    version: "2.92.35-alpha",
    date: "2026-07-06",
    highlights: [
      "Under-the-hood tidying. The shared logic behind PM's cloud connectors — how a Drive or OneDrive sync starts, stops, resumes and reports, and how each account row is drawn — now has an automated test net, so a future change there can't quietly break it.",
    ],
  },
  {
    version: "2.92.34-alpha",
    date: "2026-07-06",
    highlights: [
      "Under-the-hood tidying (developer-side only). PM's commit-time checks now also catch a forgotten version bump, its secret scanner now sweeps the whole project history rather than just the current files, and there's a new command for measuring test coverage. Nothing you'll see changes.",
    ],
  },
  {
    version: "2.92.33-alpha",
    date: "2026-07-06",
    highlights: [
      "Under-the-hood tidying. PM now has a test that upgrades a database created by a much older version all the way to the current one, checking your chats, documents and settings come through every step intact — so a future change to how the database evolves can't quietly lose data.",
    ],
  },
  {
    version: "2.92.32-alpha",
    date: "2026-07-06",
    highlights: [
      "Under-the-hood tidying. The small helper process PM uses to read and convert your files now has automated tests covering how it talks to the rest of the app — so a future change to that plumbing can't quietly break the exchange.",
    ],
  },
  {
    version: "2.92.31-alpha",
    date: "2026-07-06",
    highlights: [
      "Under-the-hood tidying. PM's code checks are stricter now: the linter understands types well enough to flag a background task whose errors would otherwise be silently dropped, and every such spot in the app was reviewed and made explicit. The type-checker also now covers PM's own build configuration, not just the app code.",
    ],
  },
  {
    version: "2.92.30-alpha",
    date: "2026-07-06",
    highlights: [
      "Under-the-hood tidying. Two of PM's own safety checks now keep each other honest — one makes sure the two internal lists of allowed software licences stay in agreement, and another makes sure every check that runs while building PM also runs in its automated online checks, so neither can quietly fall out of coverage.",
    ],
  },
  {
    version: "2.92.29-alpha",
    date: "2026-07-06",
    highlights: [
      "Under-the-hood release-safety tidying. PM's automated checks now compile the Windows build on every change, not only when a release is cut — so a Windows-only glitch is caught early instead of slipping into an update. And the release process now double-checks that each update was signed with the exact key your installed app trusts, so a signing mix-up can never quietly ship you an update your PM would refuse.",
    ],
  },
  {
    version: "2.92.28-alpha",
    date: "2026-07-06",
    highlights: [
      "Clearer backup progress. While a backup is uploading to the cloud, the progress bar now shows a gentle “working” shimmer instead of sitting frozen at 0% (uploads don’t report a percentage). And if a backup reaches some destinations but not others — say it saves to Proton Drive but Google Drive is offline — you’ll see a plain note saying which one failed, instead of it looking like everything worked.",
    ],
  },
  {
    version: "2.92.27-alpha",
    date: "2026-07-06",
    highlights: [
      "Small fixes across the app. Dropping files onto Documents now respects the “copy photos into the vault” checkbox even if you ticked it after opening the tab. A pinboard note you turn into a document now reads cleanly everywhere, not just on the board. Switching chats quickly no longer briefly shows the wrong conversation. Clearing every model from a role and saving now sticks (it falls back to the default as the screen says). And the microphone is always released if you leave the screen mid-recording.",
    ],
  },
  {
    version: "2.92.26-alpha",
    date: "2026-07-06",
    highlights: [
      "Snappier chat and Documents. A long chat no longer re-renders every earlier message as a reply streams in, and you can scroll up to re-read while a reply is still arriving (it only auto-follows when you're already at the bottom). Opening a document from a citation fetches just that one document instead of the whole list, and the Documents table skips drawing rows you've scrolled past — both stay quick as your library grows.",
    ],
  },
  {
    version: "2.92.25-alpha",
    date: "2026-07-06",
    highlights: [
      "Smoother calendar and pinboard. Editing a pinboard note no longer writes to disk on every keystroke — PM waits until you pause typing. Your calendar's background refresh now only rewrites its stored events when something has actually changed, instead of every 15 minutes regardless. And a timed event that runs until midnight (say 8pm–midnight) shows as a full block on the day grid, not a thin sliver.",
    ],
  },
  {
    version: "2.92.24-alpha",
    date: "2026-07-06",
    highlights: [
      "Dates land on the right day. Milestone, deadline and pinboard-timeline dates could show up one day early if your computer's clock is set to a timezone behind UTC — they now render on their correct calendar day everywhere. Alongside that, some internal tidy-up: the “last synced” timestamps route through one shared helper, and the overlay dimming behind dialogs is now a single named colour.",
    ],
  },
  {
    version: "2.92.23-alpha",
    date: "2026-07-06",
    highlights: [
      "Under-the-hood tidying: PM now runs an automated test safety-net over the small pieces of its interface that format your dates and clean up displayed text, so future changes to those can't quietly break them.",
    ],
  },
  {
    version: "2.92.22-alpha",
    date: "2026-07-06",
    highlights: [
      "Under-the-hood security tidying. Your vault passphrase is now scrubbed from PM's working memory the moment it's finished with, instead of lingering there. And PM's interface layer can now only read the handful of look-and-feel settings it's meant to (theme, layout, and the like) — never other internal values.",
    ],
  },
  {
    version: "2.92.21-alpha",
    date: "2026-07-06",
    highlights: [
      "Calmer startup. When PM opens, several background catch-up jobs — re-indexing chats, refreshing running summaries and titles — used to all fire at the same moment right after unlock, making the first few seconds work harder than needed (especially waking the computer from sleep). They now take turns, so nothing piles up at once. There's also a behind-the-scenes build-tuning change whose effect we'll measure before the next release.",
    ],
  },
  {
    version: "2.92.20-alpha",
    date: "2026-07-06",
    highlights: [
      "Messier files import cleanly. A spreadsheet or CSV exported from Excel in the older Windows text encoding used to fail to import (accented names and symbols tripped it up) — PM now reads those correctly. If the optional read-text-in-images component is broken or missing, importing a photo now still records its date, location and dimensions instead of failing the whole photo. And an accidentally-enormous file (say a multi-hundred-megabyte text file) is now turned away with a clear message rather than risking PM running out of memory.",
    ],
  },
  {
    version: "2.92.19-alpha",
    date: "2026-07-06",
    highlights: [
      "More honest backups. If you back up to more than one place (say Proton Drive and Google Drive) and one of them quietly starts failing, PM no longer treats everything as fine just because the other succeeded — it now records each destination's last successful backup on its own, so a stale one can be spotted (a visible warning is coming next). And disconnecting your Google account now also switches off Google-Drive backups, instead of leaving them to fail silently against an account PM can no longer reach.",
    ],
  },
  {
    version: "2.92.18-alpha",
    date: "2026-07-06",
    highlights: [
      "Two fixes for what shows up in Focus. Reminders for recurring events now come back for each new occurrence: if you dismissed a “prepare ahead” nudge for, say, a weekly standup, PM used to read that as “never remind me about this event again” — it now correctly brings the nudge back for next week’s while staying quiet about the one you already handled. And a project that has deadlines but no documents yet no longer vanishes from Focus — its milestones surface, and earn their deadline reminders, just like a project you’ve already added files to.",
    ],
  },
  {
    version: "2.92.17-alpha",
    date: "2026-07-06",
    highlights: [
      "Snappier search on big pastes, and a sturdier index fingerprint. Dropping a very large block of text into a search or chat used to make PM briefly grind while it built a keyword query out of every single word — that keyword step is now bounded, so search stays quick no matter how much you paste. And under the hood, the fingerprint PM uses to decide when your search index needs rebuilding now covers two settings it was missing, so a future change to how results are ranked or text is tokenised will correctly offer a one-time rebuild instead of quietly drifting out of sync.",
    ],
  },
  {
    version: "2.92.16-alpha",
    date: "2026-07-06",
    highlights: [
      "Sturdier chat history. When PM saves a message you or it just sent, it writes that message to a file on disk first — that file is the real record everything else is rebuilt from. If that one write ever hiccups (a momentarily locked or full drive), the message still lived in PM's working memory, but a later rebuild of the search index — which trusts the on-disk files — could quietly drop it. PM now double-checks, before it indexes, that every recent message is actually in its file, and re-writes any that slipped through. Nothing said in a conversation can fall between the two.",
    ],
  },
  {
    version: "2.92.15-alpha",
    date: "2026-07-06",
    highlights: [
      "Under-the-hood chat tidying. When you hit “Compress” to free up room in a long conversation at the same moment PM was already tidying that chat’s running summary in the background, the two could briefly step on each other and record the same stretch twice — that can no longer happen; whoever finishes first wins and the other steps aside cleanly. And if the document search that grounds an answer in your files ever fails, PM now leaves a note in its logs instead of quietly answering as if you had no documents at all.",
    ],
  },
  {
    version: "2.92.14-alpha",
    date: "2026-07-06",
    highlights: [
      "Steadier connector indexing. If a tracked folder ever changed so fast that the live watcher lost track of a burst of edits, PM used to quietly fall behind until you synced by hand — it now notices and re-checks that folder on its own. And when Google is briefly rate-limiting or having a hiccup, an indexed Google Sheet no longer gets stuck showing a “reconnect this account” note when nothing is actually wrong; it stays findable by name and fills its details back in on the next clean sync.",
    ],
  },
  {
    version: "2.92.13-alpha",
    date: "2026-07-06",
    highlights: [
      "Under-the-hood responsiveness. Making a backup or exporting your data used to briefly tie up the app while it took a consistent snapshot of your library, and the very first sync after installing an update could do the same while it set up the document engine. Both now happen off to the side, so the app stays smooth and responsive even while a large backup is running.",
    ],
  },
  {
    version: "2.92.12-alpha",
    date: "2026-07-05",
    highlights: [
      "Tidier, more reliable data removal. Two fixes to \"Remove PM data\": if you ever forget your vault passphrase, the removal now still clears your saved keys and secrets instead of stopping short — so you're never locked out of erasing your own data. And when you preview a restored backup, the temporary copy it creates is now cleaned up automatically on the next start and fully removed when you erase PM, so an old backup's contents never quietly linger on disk.",
    ],
  },
  {
    version: "2.92.11-alpha",
    date: "2026-07-05",
    highlights: [
      'Sturdier re-indexing for cloud- and folder-indexed files: in two rare cases where PM was interrupted at exactly the wrong moment — a crash mid-way through indexing a cloud file, or right after turning an indexed file into a full local copy — a later "Re-index everything" could quietly drop that item or keep reporting it as failed. Both now self-heal on the next start, so nothing is silently lost. This is behind-the-scenes robustness; nothing you do changes.',
    ],
  },
  {
    version: "2.92.10-alpha",
    date: "2026-07-05",
    highlights: [
      "Under-the-hood tidying: the Google Drive, OneDrive, and local-folder connector screens now share one set of building blocks — the account/folder list, the sync progress bar, the post-sync summary, and the Stop control — instead of three near-identical copies, so a fix or polish to one lands for all three. Nothing about how connecting or syncing works has changed; this is purely house-keeping.",
    ],
  },
  {
    version: "2.92.9-alpha",
    date: "2026-07-05",
    highlights: [
      "Sturdier cloud sign-in and sync: a brief Google Drive rate-limit no longer makes PM treat a healthy account as disconnected, and two things refreshing an account's sign-in at the same moment can no longer leave it stuck (most likely to have affected Microsoft/OneDrive). Under the hood, the Google and Microsoft sign-in flows now share one piece of plumbing.",
    ],
  },
  {
    version: "2.92.8-alpha",
    date: "2026-07-05",
    highlights: [
      "Steadier cloud sync on very large accounts: when Google Drive or OneDrive is too big to list in a single pass, PM now keeps what it found and picks up the rest on the next sync — instead of, in rare cases, removing files it simply hadn't reached yet, or a very large OneDrive not syncing at all. Under the hood, Drive syncing and Drive backup now share one piece of Google plumbing.",
    ],
  },
  {
    version: "2.92.7-alpha",
    date: "2026-07-05",
    highlights: [
      "Under-the-hood tidying: Google Drive and OneDrive now share a single sync engine instead of two near-identical copies, so a fix or improvement to cloud syncing lands once for both. Nothing about how syncing works has changed — this is purely house-keeping.",
    ],
  },
  {
    version: "2.92.6-alpha",
    date: "2026-07-05",
    highlights: [
      "Under-the-hood tidying: the Google Drive, OneDrive, and local-folder sync engines moved out of the big command file into their own modules, so the connector code is easier to find and change. Nothing about how syncing works has changed — this is purely house-keeping.",
    ],
  },
  {
    version: "2.92.5-alpha",
    date: "2026-07-05",
    highlights: [
      "Under-the-hood tidying: the Google Drive, OneDrive, and local-folder sync engines now share one copy of their common plumbing (the single-flight/crash-recovery wrapper and the indexing step) instead of three near-identical copies. Syncing behaves exactly as before — this just means future fixes land in one place.",
    ],
  },
  {
    version: "2.92.4-alpha",
    date: "2026-07-05",
    highlights: [
      "If a Google Drive or OneDrive sync hit a snag part-way through — a file it couldn't fetch or index — the connector could still show as fully synced and quietly leave those files out. Now a sync that runs into an error is flagged so you can see it needs another try, and the files it couldn't reach are picked up automatically on the next sync instead of being skipped.",
    ],
  },
  {
    version: "2.92.3-alpha",
    date: "2026-07-05",
    highlights: [
      "A cloud or local-folder sync that hit an unexpected error could quietly leave that connector unable to sync again until you restarted PM. Now it always recovers on its own — the next sync just runs normally.",
    ],
  },
  {
    version: "2.92.2-alpha",
    date: "2026-07-05",
    highlights: [
      "Under-the-hood tidying: the Google Drive and OneDrive connectors now share a single piece of folder-sync reconciliation logic instead of keeping near-identical copies. Sync behaves exactly as before — this just means future fixes land in one place rather than needing to be applied twice.",
    ],
  },
  {
    version: "2.92.1-alpha",
    date: "2026-07-05",
    highlights: [
      "A project's chat now shows the same context meter as the main chat. When a scoped conversation starts filling the model's context window, you get the same at-a-glance gauge and the same Compress / Continue / switch-to-a-bigger-model options — instead of silently running into the limit.",
    ],
  },
  {
    version: "2.92.0-alpha",
    date: "2026-07-05",
    highlights: [
      "Search now works properly in Chinese, Japanese, Korean and other languages that don't put spaces between words. If your vault's search language is set to multilingual, keyword search finds these documents instead of quietly falling back to a weaker match — and PM offers a one-time re-index to bring your existing documents across. Vaults searching in English are unaffected.",
      "Short messages in any language are kept. A brief note in Chinese, Russian or another non-Latin script is no longer mistaken for small talk and skipped — it's indexed and learned from just like an English one.",
    ],
  },
  {
    version: "2.91.9-alpha",
    date: "2026-07-05",
    highlights: [
      "Renaming or merging a project now keeps everything attached to it. Before, a rename could quietly strip a project of its milestones, deadline, status, saved chats and activity history — now they all move across to the new name.",
      "PM stays out of your way while you work. Background housekeeping — indexing, summaries, backups and the daily catch-up — now waits for a genuine pause instead of kicking in while you're reading, scrolling, triaging or editing.",
      "The app opens faster. The first screen no longer waits on a calendar sync or a chain of startup checks before it appears — your projects show up right away, and the calendar fills in a moment later.",
      "Backups are sturdier. A large Google Drive backup now uploads in pieces — gentler on memory, and it resumes instead of restarting if the connection blips — and a Proton backup can no longer hang forever: it times out on its own, and Cancel now works mid-transfer.",
    ],
  },
  {
    version: "2.91.8-alpha",
    date: "2026-07-05",
    highlights: [
      "Startup is more forgiving. If PM can't open your vault the instant it launches — usually because antivirus or Windows Search is briefly scanning the file — it no longer closes with an error. You get a friendly “try again” screen instead, and your documents are untouched.",
      "Sharing a vault across two Windows profiles is safer: if one computer goes to sleep and the other takes over as the writer, the sleeping one now steps aside cleanly when it wakes, instead of both trying to write at once. And switching a vault back to private-to-this-device now recovers reliably if it's interrupted partway, rather than getting stuck on the next launch.",
    ],
  },
  {
    version: "2.91.7-alpha",
    date: "2026-07-05",
    highlights: [
      "Reliability fix for the document engine: a very large file — a big log or text dump — no longer jams PM's search, indexing, transcription and map. Previously one oversized file could quietly freeze all of them until you restarted; now the big file is handled in pieces and everything keeps working.",
      "If the document engine ever does get stuck — a first-time model download that stalls, an operation that hangs — PM now notices and recovers on its own instead of staying frozen until the next restart.",
    ],
  },
  {
    version: "2.91.6-alpha",
    date: "2026-07-05",
    highlights: [
      "Chat reliability: if a reply ever fails to come back — a dropped connection, a provider hiccup — that conversation is no longer stuck. Just send your next message and it carries on as normal; previously the only way out was deleting the whole chat.",
      "Long answers are no longer cut off. A reply that takes several minutes to write now streams to the end instead of stopping partway; PM only gives up if the connection actually goes silent.",
    ],
  },
  {
    version: "2.91.5-alpha",
    date: "2026-07-05",
    highlights: [
      "Safety fix for “Remove PM data”: if you had moved your vault into a folder that also held your own files, removing PM’s data now clears only PM’s files and leaves everything else untouched. Previously it could delete that whole folder. If PM had the folder to itself, the now-empty folder is tidied away as before.",
    ],
  },
  {
    version: "2.91.4-alpha",
    date: "2026-07-05",
    highlights: [
      "Reliability fix: the preferences you teach PM for a specific project are now always kept when PM refreshes its internal project index. Previously a rare internal rebuild could clear them. Nothing changes in how you use PM — your taught preferences just stay put.",
    ],
  },
  {
    version: "2.91.3-alpha",
    date: "2026-07-05",
    highlights: [
      "Behind the scenes: documented a rule for future data-format changes so your organised files and folders always stay put. No change to how you use PM.",
    ],
  },
  {
    version: "2.91.2-alpha",
    date: "2026-07-04",
    highlights: [
      "Behind the scenes: refreshed the contributor documentation that describes how PM is built. No change to how you use PM.",
    ],
  },
  {
    version: "2.91.1-alpha",
    date: "2026-07-04",
    highlights: [
      "Behind the scenes: routine maintenance updates to a batch of bundled dependencies, keeping the build tooling and app framework current. No change to the app itself.",
    ],
  },
  {
    version: "2.91.0-alpha",
    date: "2026-07-04",
    highlights: [
      "Uninstalling PM now tidies up after itself. The regular uninstaller clears the large, re-downloadable components too — the document engine, the enhanced-map and photo-text add-ons, and the speech model — so removing PM no longer leaves gigabytes behind. Your vault, database and sign-ins are deliberately kept, so if you reinstall, PM picks up right where you left off.",
      "There's a new way to fully clear PM off your machine. Settings → Data & Security → “Remove PM data” lets you erase things piece by piece: the downloaded components, your vault and encrypted database, your saved keys and sign-ins, and this window's preferences. Removing your saved keys also revokes PM's access to your connected Google accounts (Microsoft is one click away, at account.live.com, since it can't be revoked from within an app). It's made hard to do by accident — you unlock the choices, review exactly what's selected and what each one means, pass a Windows Hello check if you use one, and type “Delete PM Data” to finish. Encrypted backups are left untouched on purpose, with a reminder to remove those yourself at Proton or Google Drive.",
    ],
  },
  {
    version: "2.90.0-alpha",
    date: "2026-07-04",
    highlights: [
      "Ingesting a Pinboard note now saves it into your vault as a real Markdown document — the whole note, not a short summary. So it's fully readable and searchable offline, it survives you tidying the note away, and a full Rebuild reproduces it from disk like any other document. Notes you'd already ingested under the previous version are quietly upgraded to the full document the next time you re-ingest them, keeping wherever you'd filed them.",
      "Pinboard notes gained a formatting toolbar. Under a note you're editing there are buttons for bold, italic, a heading, and bullet / numbered / checklist lists — each toggles on and off, applies to the line(s) you've selected, and has a keyboard shortcut (shown when you hover). A note also renders itself whenever you're not editing it — lists, headings, checkboxes and all — and you click it to write again; the separate Edit/Preview toggle is gone.",
      "The Pinboard now fills the window. The board grows to match your screen — the notes and text stay exactly the same size, there's just more room — so a maximised window shows the whole board with no scrollbars, and shrinking the window scrolls it to its edges with proper draggable scrollbars you can grab. PM opens maximised now, too.",
      "You can resize and hide the side panels. Drag the edge of the left navigation sidebar, or a project's side panel, to set its width — and drag it all the way to the window edge to tuck it away behind a slim tab, then click the tab to bring it back. Your choice is remembered.",
      "The chat input is tidier again. The message box grows as you type a longer draft (up to a point, then it scrolls) and shrinks back when you send; the mic·box·Send cluster stays centred under your conversation with the context and Explain buttons tucked just outside it; and the context-window indicator is now a small ring that fills as a conversation grows — turning red when it's getting full — in place of a bare percentage.",
      "A handful of smaller fixes: pop-up dialogs now open below the title bar so the window's own minimise and close buttons stay reachable; in the Review queue you can click a document's title to read it in the panel while you file it; the Documents table no longer pushes the page sideways when a title is long (it trims instead); and a vertical mouse-wheel always scrolls vertically everywhere, while a wide table scrolls sideways as you'd expect.",
    ],
  },
  {
    version: "2.89.0-alpha",
    date: "2026-07-04",
    highlights: [
      "A Pinboard note can now become a real document. Hit Ingest on a note and PM takes it in the same way it takes any file — it's chunked, indexed and dropped into your Review queue to file to a project and set its importance, after which it turns up in Documents, Focus, chat and search like everything else. The note shows “In review” until you file it, then “Filed · project”. Edit the note later and one “Re-ingest” updates the same document while keeping where you filed it. The ingested document is its own copy from that point on — so if you tidy the note away, what you ingested stays put.",
    ],
  },
  {
    version: "2.88.0-alpha",
    date: "2026-07-04",
    highlights: [
      "A Pinboard timeline can now track a real project. Link one (pick an existing project or type a new name) and the card shows that project's actual milestones laid out on a line, earliest to latest — each a dot with its date and label. Add, rename, re-date, tick off or remove a milestone here and it writes straight through to the project, so the very same deadlines show up in your daily briefing and the project's Focus panel — and a milestone you add there appears on the timeline too (it refreshes when you come back to the window). Dates synced from a linked calendar event stay read-only. Unlink any time to go back to a plain scratch timeline; the project keeps its milestones. Timelines with nothing linked behave exactly as before.",
    ],
  },
  {
    version: "2.87.0-alpha",
    date: "2026-07-04",
    highlights: [
      "Pinboard notes now do lists — and formatting. Start a line with . or - for a bullet, 1. for a numbered list, i. for roman numerals, > for an arrow/quote, or [] for a checkbox, and pressing Enter carries the list on (the next bullet, the next number, a fresh checkbox); Enter on an empty item ends the list. Each note has an Edit/Preview toggle: preview renders it properly — headings, bullets, numbered and roman lists, checkboxes and quotes — and double-clicking the preview drops you back into editing. A note is still just plain text underneath, so nothing is locked into a special format.",
      "The Timeline card on the Pinboard is now available on every density, not just Standard and Power — so a minimal setup can sketch dated plans too.",
    ],
  },
  {
    version: "2.86.0-alpha",
    date: "2026-07-04",
    highlights: [
      "Help mode now explains a lot more of PM. Turn it on in Settings → General and hover — the whole Pinboard is covered for the first time (the board, notes, timelines and the tint colours), as is the Calendar tab (the view, how to move around it, and choosing which calendars show). A batch of spots that used to highlight but say nothing when hovered — the “say what you mean” box on your home screen, the project side panel and its milestones/files split, OneDrive and local-folder connectors and their sync results, encrypted backup, vault sharing, the Microsoft sign-in, and the idle/resumed markers in chat — now all explain themselves. Nothing about how PM works changed; there's just far less of it left unexplained.",
    ],
  },
  {
    version: "2.85.0-alpha",
    date: "2026-07-04",
    highlights: [
      "The chat input is tidier. The two strips that used to stack above the message box — the context-window meter and “Explain retrieval” — are now compact buttons on the input row itself, each opening the same panel just above it when you need it. Same information and controls, less space taken up, so more of the window is your conversation. The context button still turns red and nudges you when a conversation is filling the model's window, and Explain retrieval still only appears on the Standard and Power presets.",
    ],
  },
  {
    version: "2.84.0-alpha",
    date: "2026-07-04",
    highlights: [
      "PM has a new default look: a calm, monochrome dark theme. Its base is Eigengrau — the deep near-black your eyes settle on in true darkness — with a soft off-white text and highlights (eased back from pure white so it's easy on the eyes), and no colour tinting the buttons, borders or panels. The colours that actually mean something stay in colour: the memory map's project hues, and the status tags on your projects (Due soon, Blocked, and the rest). You'll find it in Settings → General → Appearance as the first Accent swatch under the Slate style (the dark circle with a white rim); the original coloured Slate accents are still right beside it, and your other styles (Editorial, Terminal) are untouched. If you'd already picked your own look, nothing changes — this is only the starting point for a fresh install.",
      "Mode now has four choices instead of two. Alongside Light and Dark there's System, which follows your computer's own light/dark setting and flips when it does, and Auto, which follows the sun where you are — light through the day, dark after sunset — and switches itself as the day turns. Auto works out sunrise and sunset entirely on your device from your timezone, with no location permission and nothing sent anywhere; if you'd like it exact, you can type your own coordinates under the Mode picker, and it shows you when the next switch is due.",
    ],
  },
  {
    version: "2.83.0-alpha",
    date: "2026-07-03",
    highlights: [
      "You can now read your documents right inside PM. Click any item in the Documents list — or a file in a project, or a source a chat answer cites — and it opens in a reading panel beside it, without hunting for it first. Your notes, spreadsheets and saved conversations render cleanly with headings, lists and tables, and a photo shows its original image (or, if you didn't save the image, the text PM read from it). For an item you keep in Google Drive, OneDrive or a folder on your own computer, the reader now fetches and shows the full text (falling back to the offline summary if the source can't be reached), plus one button to open the original at its source — opening the web link, or revealing the file in your file manager. The old “Open in Drive” link and the separate “Show full text” button have both folded into this panel. Drag the panel's left edge to widen it up to half the window, and it now sits below the title bar so the window's own controls stay within reach.",
      "Sources are now clickable everywhere they appear. When a chat answer lists the documents it drew from — in the main chat or a project's own chat — click one to open it in the reader and see exactly what PM was working from, then close it and carry on.",
      "For power users: turn the density up to Power and the reader gains a “Show chunks” view that paints the exact boundaries PM used to split a document for search, shading each chunk as its own band so you can see how a file was divided from top to bottom (for anything with full text, including your cloud and folder items). It's read-only, and (in developer mode) can show the raw source with the same boundaries. Nothing is re-indexed or changed by looking.",
    ],
  },
  {
    version: "2.82.0-alpha",
    date: "2026-07-03",
    highlights: [
      "You can now point PM at a folder on your own computer. Open Settings → Connectors → This device → Add a folder, pick one, and PM indexes the documents inside it — so what's in your files turns up in search, right alongside your Google Drive and OneDrive. As with those, nothing is copied into PM: it only reads each file to index it, and the file stays exactly where it is. Once a folder is added PM watches it and keeps it current as you work — edit a file and it re-indexes within seconds, rename or move one and it keeps its place, delete one and it drops out of your results (still findable by name). Each folder shows how many files it's indexed and when it last synced, with Sync now and Remove; removing a folder just stops the watching — the items you've already indexed stay findable. This completes local folders as a source.",
    ],
  },
  {
    version: "2.81.0-alpha",
    date: "2026-07-03",
    highlights: [
      "Groundwork (cont.): PM now watches the folders you've pointed it at and keeps them indexed on its own — save a file and its contents are searchable within a few seconds, rename or move one and it keeps its place, project and tags, delete one and it quietly drops out of your results (still findable if you search for it by name). It also catches up whenever you reopen PM, so anything you changed while it was closed is folded in. It never keeps a copy of your files — it only reads them to index, exactly as before. The button to add a folder is the last piece, coming next.",
    ],
  },
  {
    version: "2.80.0-alpha",
    date: "2026-07-03",
    highlights: [
      "Groundwork: PM can now index a folder on your own computer — the same way it already does for Google Drive and OneDrive, so what's inside your files turns up in search without copying anything into PM. This first step builds the engine: it walks a folder you point it at, notices what's new, changed, moved or deleted (a rename keeps a file's place and its project and tags, rather than reading as a brand-new file), and opens each file only to index it — never keeping a copy. Nothing on screen yet; the button to add a folder, and live watching as files change, land in the next updates.",
    ],
  },
  {
    version: "2.79.0-alpha",
    date: "2026-07-03",
    highlights: [
      "There's now a box right under your daily briefing where you can just say what you mean, in plain words. Tell it something's handled (“the deck is done”) and PM marks that item off for you; tell it how you'd like to be nudged (“stop reminding me so early”) and it remembers that as a lasting preference; or ask a question and it opens a chat to answer — no menus, no clicking the right item first. Anything that would cross something off asks you to confirm first, so nothing disappears by accident.",
      "Chat now shares the same “what needs your attention” layer as your briefing. Ask “am I ready for tomorrow?” or “what's pressing this week?” and it answers from your actual flagged deadlines, events and prep — and a project's own chat sees just that project's items. This wraps up the groundwork from the last few updates: one honest, shared picture of what matters, that the briefing, chat and that new box all read from.",
      "Marking a deadline done from your briefing now also ticks it off inside the project — before, the two could disagree, with the briefing showing it handled while the project still listed it as pending. They're the same fact now, so the project's status updates to match in one step. And if you got it wrong, un-ticking the milestone brings the reminder back to your briefing, so a mistaken “done” is easy to undo.",
      "The box under your briefing now keeps a suggestion you haven't acted on yet — switch to another tab and back and it's still waiting for your confirm, instead of vanishing.",
    ],
  },
  {
    version: "2.78.0-alpha",
    date: "2026-07-03",
    highlights: [
      "Groundwork (cont.): items in your “what needs your attention” layer can now be marked done — and PM treats that as a decision it remembers, not a line it erases. A resolved item drops out of the briefing, and a later re-scan can't quietly bring it back; if you'd already prepared for an event, its “happening today” note reads “you're prepared — file's here” with a link, instead of nagging you to prepare again. The button to mark things done straight from the briefing lands in the next update — this step puts the resolution rules and the honest record underneath them in place.",
    ],
  },
  {
    version: "2.77.0-alpha",
    date: "2026-07-03",
    highlights: [
      "Your daily briefing now stands on a real “what needs your attention” layer: PM works out the pressing items first — an overdue or approaching milestone deadline, an event happening today, something to prepare for in the next few days — and the briefing describes those, instead of the model free-associating over your projects each morning. It stays honest across the day too, so an approaching deadline quietly becomes “overdue” once it passes, and refreshing the briefing won't reshuffle the facts underneath it.",
    ],
  },
  {
    version: "2.76.0-alpha",
    date: "2026-07-03",
    highlights: [
      "Groundwork: PM is starting to build a proper “what needs your attention” layer under the daily briefing — things like an approaching deadline or a presentation happening today become tracked items with their own memory, rather than a paragraph the model rewrites from scratch each day. Nothing changes on screen yet; this first step just lays the foundation so the briefing (and, later, chat) can point to the same stable to-dos and let you mark them done.",
    ],
  },
  {
    version: "2.75.0-alpha",
    date: "2026-07-03",
    highlights: [
      "New: move a chat into a project — or back to global. Hover any conversation in the sidebar and use the new 📁 button to file it under a project (its search then narrows to that project's files) or send it back to global (searches everything). Past messages stay put; only where the chat lives changes, and it takes effect on your next message.",
    ],
  },
  {
    version: "2.74.0-alpha",
    date: "2026-07-03",
    highlights: [
      "Under the hood: the quiet activity signal now tidies itself — older entries are compacted into small daily summaries once a day while the app is idle, so it stays lean and never grows without bound. Nothing to see; it just keeps house.",
    ],
  },
  {
    version: "2.73.0-alpha",
    date: "2026-07-03",
    highlights: [
      "Groundwork (cont.): the same quiet activity signal now also notices when you file a document into a project or change a project's milestones. Still nothing new on screen — it's building the history that will power smarter surfacing (which projects are “hot” right now) in a later update.",
    ],
  },
  {
    version: "2.72.0-alpha",
    date: "2026-07-03",
    highlights: [
      "Groundwork: PM now quietly notes which projects you're actively working in — a message in a project's chat counts as engaging with it. Nothing changes on screen yet; this is the memory that will power smarter surfacing (which projects are “hot” right now) in a later update.",
    ],
  },
  {
    version: "2.71.0-alpha",
    date: "2026-07-03",
    highlights: [
      "New: “Import fully” for indexed Google Sheets. When a Sheet from your Drive is indexed by pointer only, its row in Documents now has an Import fully button — click it and PM downloads the whole spreadsheet (every tab), reads it as a proper local spreadsheet, and makes all of its cells searchable, keeping wherever you'd already filed it. The original stays linked in Drive, and PM won't re-add it as a duplicate on later syncs.",
    ],
  },
  {
    version: "2.70.0-alpha",
    date: "2026-07-03",
    highlights: [
      "New: Google Sheets from your connected Drive are now indexed the smart way — a lightweight pointer with the spreadsheet's tab names and column headers plus a link to open it in Drive, instead of dumping the whole grid into search. Sheets stay findable and their link always works. To enable the richer tab/header details on a Google account you connected earlier, use the new “Reconnect for Sheets” button in Settings → Connectors (a quick re-approval in your browser that adds read-only Sheets access) — until then those sheets are indexed by name only. Want a sheet's actual contents searchable? Importing it fully is coming next.",
    ],
  },
  {
    version: "2.69.0-alpha",
    date: "2026-07-03",
    highlights: [
      "New: spreadsheets are indexed properly now. Drop in an .xlsx, .xls, or .csv and PM reads it as a spreadsheet — not as one giant table mangled into search — with an overview of each sheet (its columns, their types, the row count, and any date range) plus each row indexed on its own with the column names folded in, so a search finds the exact row and still knows what every value means. Small sheets stay together as one entry; very large sheets are indexed up to a sensible limit (the overview always tells you the true total). Multi-sheet workbooks are handled sheet by sheet. Nothing to turn on — it just happens on ingest.",
    ],
  },
  {
    version: "2.68.0-alpha",
    date: "2026-07-03",
    highlights: [
      "New: projects you haven't set a priority for can now show one anyway. If other projects depend on a project — it's a parent of them, or they're blocked on it — the Focus view now works that out on its own and shows a priority tag marked “(auto)”, so the projects other work is waiting on stand out even before you've triaged them. Set a priority yourself in Triage at any time and it takes over, exactly as before.",
    ],
  },
  {
    version: "2.67.0-alpha",
    date: "2026-07-03",
    highlights: [
      "New: back up to Google Drive too, alongside Proton Drive. In Settings → Backup you can now grant PM permission to save its encrypted backups to your own Google Drive — a one-time approval in your browser that only lets PM manage its own “Personal Manager Backups” folder, never the rest of your Drive. Your automatic-backup schedule is now shared: pick a frequency once and turn on each destination (Proton, Google, or both) you want it to reach — every scheduled run fans out to all of them, and each keeps its own last-N history (older backups move to Google Drive’s Trash, so nothing is truly deleted). You can save to Google Drive on demand and restore any backup from it, exactly like Proton.",
    ],
  },
  {
    version: "2.66.0-alpha",
    date: "2026-07-02",
    highlights: [
      "New: automatic backups to Proton Drive, on a schedule. In Settings → Backup you can now set PM to back up to your Proton Drive every day, week, or month — no clicking required. Choose how many backups to keep (older ones move to your Proton Trash, so nothing is ever truly deleted). Because scheduled backups run unattended, PM stores your backup passphrase in your operating system’s keychain — this is an explicit opt-in, and you can turn it off (and forget the passphrase) any time. Automatic backups only run when your vault is unlocked, you’re not in the middle of something, and Proton Drive is connected.",
    ],
  },
  {
    version: "2.65.0-alpha",
    date: "2026-07-02",
    highlights: [
      "New: back up straight to your own Proton Drive. If you have Proton’s official Drive command-line tool installed, the Settings → Backup tab can now connect to your Proton account (sign-in happens in your browser — PM never sees your Proton password) and push the same encrypted, restore-anywhere backup to a “Personal Manager Backups” folder in your Drive. You can see what’s already backed up there and restore any of them right from PM. Don’t have the tool? PM detects that and links you to the official download — it never bundles or downloads it for you. (Automatic, scheduled backups are coming next.)",
    ],
  },
  {
    version: "2.64.0-alpha",
    date: "2026-07-02",
    highlights: [
      "New: encrypted backups you can actually restore anywhere. A new Settings → Backup tab saves your whole vault — notes, database, and settings — as a single, compressed, password-protected file. Unlike “Export all data” (which only opens on this computer), a backup is self-contained: move it to another machine, enter your passphrase, and restore. Restoring is safe by design — it unpacks and verifies everything into a new folder first, so your current vault is never touched until you choose to switch to the restored one. Keep your backup passphrase somewhere safe: there’s no way to recover a backup without it. (Backing up straight to Proton Drive, and on a schedule, is coming next.)",
    ],
  },
  {
    version: "2.63.0-alpha",
    date: "2026-07-02",
    highlights: [
      "Setting up on a Mac is now truly one-click. If your Mac doesn’t already have a recent Python, PM downloads a private copy just for itself the first time you set up the document engine — no Terminal, no Homebrew, no “install Python first”. You’ll see a progress bar while it downloads, and it only happens once. (Windows already ships with everything built in; nothing changes there.)",
    ],
  },
  {
    version: "2.62.0-alpha",
    date: "2026-07-02",
    highlights: [
      "Filing suggestions now take a hint from your Drive folders. When PM proposes a project for a file synced from Google Drive, it quietly considers the folder the file was found in — so a document sitting in your “Taxes 2025” folder is more likely to be suggested under the right project. It’s only ever a hint: every document still passes through your review before anything is filed, and the folder name never changes how the file is searched.",
    ],
  },
  {
    version: "2.61.0-alpha",
    date: "2026-07-02",
    highlights: [
      "Calendar polish and a thorough reliability pass. The Day scale now opens on your daytime hours and fills the pane (and still scrolls to the small hours), the 24-hour scale no longer leaves a sea of empty space, and the Year view scales to fill the window in every look. Multi-day events read as a proper bar, and the settings pop-up no longer shows the “now” line through it.",
      "Fewer events quietly go missing. Daily repeating events from an iCal subscription now show for the whole year (not just the first ~12 months), an event that’s still running from earlier stays on today’s agenda, a busy month shows a “+N more” instead of dropping overlapping multi-day events, and the same event synced from two calendars appears once. Outlook repeating events now show every occurrence, and all-day events no longer disappear halfway through their own day.",
      "Steadier under the hood. Recurring events land on the right time across daylight-saving changes and whatever machine synced them, a calendar that partly fails to sync is flagged rather than shown as fine, and disconnecting an account reliably clears its saved sign-in. The clock line and “today” now advance on their own.",
    ],
  },
  {
    version: "2.60.0-alpha",
    date: "2026-07-02",
    highlights: [
      "The Calendar now speaks Terminal. Switch PM to the Terminal look and every view — Month, Week, Day, Year, and Agenda — turns into a crisp monospace layout: a “~/pm ❯ cal” status line, square day markers, and green kept strictly for today. (Week and Day read as a tidy day-by-day agenda rather than a pixel grid, which suits the mono style.)",
      "Small touches everywhere else, too. Page through time from the keyboard — ← and → step by period and “t” jumps back to today — each view fades in gently as you switch (and holds still if you’ve asked for reduced motion), and PM now tells you when you’ve paged past the stretch of time it keeps synced.",
    ],
  },
  {
    version: "2.59.0-alpha",
    date: "2026-07-02",
    highlights: [
      "The Calendar grows into a real calendar. Alongside Agenda, there are now Month, Week, Day, and Year views — switch between them from the header, and page through time with the arrows or by picking a date straight off the pop-up mini-calendar.",
      "Week and Day show a proper time grid: events sit at their real hours, side by side when they overlap, with a live “now” line on today and an all-day strip up top. Choose how tall the grid runs — business hours, the whole day, or a compact 24-hour view.",
      "Month lays out your weeks with multi-day events flowing across as bands and the rest as tidy per-day chips (they shrink to coloured dots when you keep things minimal). Year gives you all twelve months at a glance — click any day to dive in. Every view stays colour-coded by source and themed to match the rest of PM.",
    ],
  },
  {
    version: "2.58.0-alpha",
    date: "2026-07-01",
    highlights: [
      "Your calendars, together at last. A new Calendar tab gathers every calendar you’ve connected — Google, Outlook, and iCal — into one read-only agenda, colour-coded by source. Show or hide any calendar on the fly (it’s instant, nothing re-syncs), step through by month, and hit Refresh for the latest; PM also keeps the view quietly up to date in the background.",
      "Under the hood, PM now mirrors a full year ahead (and the month behind), not just the next three weeks — so the calendar has the room to grow. Month, week, day, and year views are coming next.",
    ],
  },
  {
    version: "2.57.1-alpha",
    date: "2026-07-01",
    highlights: [
      "Chat polish, under the hood. Long general chats now genuinely stay cheap as they grow (the running summary is cached properly again), and a chat that runs on never balloons its cost even if the summary falls behind.",
      "Your conversations survive a re-index intact. Rebuilding search now keeps each chat’s identity — answers still jump to the exact turn they drew from, and a chat you filed into a project (or archived) stays exactly where you put it.",
      "A preference PM notices in chat truly waits for you. It stays a suggestion in Teach and never steers a reply until you’ve kept it. Plus smaller fixes: the context meter reads true even when a backup model answers, deleting a conversation leaves nothing behind, and auto-naming an old chat no longer bumps it to the top of the list.",
    ],
  },
  {
    version: "2.57.0-alpha",
    date: "2026-07-01",
    highlights: [
      "Chat can now show its work. Open “Explain retrieval” under any conversation to see exactly which of your notes it pulled and how each one scored. Drag one dial — depth — to widen the pool the ranker gets to weigh (not just how many results show), then hit “Use this depth” to make it stick.",
      "Not sure why it missed something? Describe it in plain words and PM will tell you what to try and why — it only ever suggests, and never changes a setting on its own; the dial stays in your hands. (Shows up on the fuller layouts; hide it any time from Appearance.)",
    ],
  },
  {
    version: "2.56.0-alpha",
    date: "2026-07-01",
    highlights: [
      "You can now delete a past conversation: hover it in the sidebar, hit the trash, confirm — and it’s gone for good, along with its messages and everything it had added to search. Nothing’s left dangling behind the scenes.",
    ],
  },
  {
    version: "2.55.0-alpha",
    date: "2026-07-01",
    highlights: [
      "Chats now wear their colours everywhere they show up: a little 💬 marks them in your review inbox and in a project’s file list, so at a glance you can tell a conversation apart from a document you saved.",
      "When PM suggests a preference it picked up from something you said in chat, Teach now says “Suggested from chat” — so you know where the idea came from before you decide whether to keep it.",
    ],
  },
  {
    version: "2.54.0-alpha",
    date: "2026-07-01",
    highlights: [
      "PM now notices when you state a preference in a chat — say “I always want dates as DD-MM-YYYY” or “keep replies short for the Atlas project” and it quietly turns up in Teach as a suggestion you can keep with one click (or ignore). It only ever picks up things you actually said, never guesses, and it never applies one until you’ve kept it.",
    ],
  },
  {
    version: "2.53.0-alpha",
    date: "2026-07-01",
    highlights: [
      "Your conversations now sort themselves like everything else. A chat you start inside a project files itself there straightaway (marked important, kept close to hand), while a general chat drops into your review inbox for you to place — the same approve-or-correct step your documents already get.",
      "Nothing stays wrongly buried: pick a chat back up and let it turn into a real discussion, and PM re-evaluates it — a throwaway you'd archived comes back out of the archive, and an already-filed chat returns to review so it can be re-placed.",
      "Archiving now truly tidies up — an archived chat or document drops off the semantic Map, so the picture stays about what matters (it's still there in search whenever you go looking).",
    ],
  },
  {
    version: "2.52.0-alpha",
    date: "2026-07-01",
    highlights: [
      "When an answer draws on one of your past chats, it now says so — the source reads “from [the chat’s name], [date]” instead of looking like a plain file. Click it and PM jumps you straight back to that conversation, scrolled to the exact turn it drew from, so you can see the full context behind the answer.",
    ],
  },
  {
    version: "2.51.0-alpha",
    date: "2026-07-01",
    highlights: [
      "Each project now keeps its own chat history, right where you'd expect it — in the left sidebar under Conversations, just like the main chat. Open a project and its past conversations are listed there; click any one to pick it back up.",
      "The project panel on the right now gives Milestones and Files their own space, split by a divider you can drag to suit each project. Where you set it is remembered everywhere, even on another computer.",
      "Starting fresh is one click (or one word): the “+ New” button — or just typing /new (or /done) — tucks the current conversation into history and opens a clean one, without losing anything. And reopen a project chat that's been quiet for more than a day and PM gently offers to start a new one, so a fresh train of thought doesn't get tacked onto yesterday's.",
    ],
  },
  {
    version: "2.50.0-alpha",
    date: "2026-06-29",
    highlights: [
      "Your chats name themselves. After a few exchanges, PM gives a conversation a short, fitting title (using the background model, so it never slows down the chat you're having) — and you can rename any conversation just by clicking its title.",
      "Reopen an old conversation and PM marks it as resumed, with the date it was last active, so you always know whether you're picking up an older thread or carrying on a current one.",
    ],
  },
  {
    version: "2.49.0-alpha",
    date: "2026-06-29",
    highlights: [
      "Chat now shows how full the current model's context is — a quiet meter by the message box, tuned to whichever model you're using. As a long conversation approaches that model's limit, PM offers to compress it (folding the older turns into a short summary to free up room, and showing you exactly what was condensed so you can undo if needed), switch to a model with a bigger context, or simply carry on.",
    ],
  },
  {
    version: "2.48.0-alpha",
    date: "2026-06-28",
    highlights: [
      "PM now shows how full the current model's context is. Each model has its own size limit, so the meter is tuned to whichever model you've picked — giving you a quiet heads-up as a long conversation starts to fill it up. (The on-screen meter itself arrives in the next update; this release adds the model-aware measurement and the controls behind it.)",
    ],
  },
  {
    version: "2.47.0-alpha",
    date: "2026-06-28",
    highlights: [
      "Long conversations are now cheaper and sharper. Instead of resending your whole chat history every message, PM sends the recent turns word-for-word plus the running summary of everything earlier — and keeps that stable part cached, so each new message costs far less as a thread grows.",
      "PM no longer talks over itself: when it looks things up to answer you, it skips anything that's already visible in the recent conversation, so it pulls in genuinely new context from your other notes and chats rather than echoing what's on screen.",
    ],
  },
  {
    version: "2.46.0-alpha",
    date: "2026-06-28",
    highlights: [
      "Long chats now stay affordable and stay on-point. PM keeps a short, running summary of the earlier part of each conversation, updated quietly in the background as you talk — so it can remember the gist of where you've been without re-reading the whole thread every message. The full conversation is always kept word-for-word in your vault; the summary is just a lightweight memory aid PM can rebuild any time.",
    ],
  },
  {
    version: "2.45.0-alpha",
    date: "2026-06-28",
    highlights: [
      "PM now keeps up with your conversations as you go. While you're not actively chatting, it quietly indexes new back-and-forths in the background — so a long session becomes searchable bit by bit, and you no longer have to reopen the app for it to catch up. It always waits for a lull, so it never gets in the way of what you're doing.",
      'Small talk stays out of the way: a quick "thanks" or "ok" no longer clutters what PM can recall — only the parts of a conversation that actually carry something (a decision, a fact, an answer) become searchable. Your full chat is always kept either way.',
    ],
  },
  {
    version: "2.44.0-alpha",
    date: "2026-06-28",
    highlights: [
      "Your conversations with PM start becoming searchable. PM now quietly turns each completed back-and-forth into part of its memory — so a decision, fact, or preference you mentioned in chat can resurface later when it's relevant, the same way your documents do.",
      "It happens on its own and stays out of your way: when you reopen PM, it catches up on any chat that hasn't been indexed yet, in the background. (The ongoing while-you-work indexing and the skip-the-small-talk filter arrive in the very next update.)",
      "Each piece of a chat keeps its own date, so an old conversation with one fresh decision isn't treated as entirely stale — the fresh part can still surface when it matters.",
    ],
  },
  {
    version: "2.43.0-alpha",
    date: "2026-06-28",
    highlights: [
      "Groundwork for a big one: your conversations with PM are on their way to becoming a first-class, searchable source — so a decision or preference you mention in chat can be recalled later, just like a document. This update lays the quiet foundation for how a chat is saved and tracked; nothing changes in how chat looks or works yet.",
      "Under the hood, each completed back-and-forth is now kept in your vault as it happens — the same durable, rebuildable place your documents live — so once indexing arrives in the next updates, your past chats become part of what PM can draw on.",
    ],
  },
  {
    version: "2.42.0-alpha",
    date: "2026-06-28",
    highlights: [
      "Drop in photos and screenshots — the feature is now fully wired up. The first time you add a photo, PM offers to install on-device text recognition (a one-time ~70–100 MB download). You can skip it and still add the photo (indexed by its date and location), then turn it on later — your choice, no pressure.",
      "New option when adding photos: “Save a copy in the vault.” It's off by default — PM just references your photos where they are — but flip it on to keep a copy inside PM (handy for screenshots you delete after), and it follows your vault's encryption. Re-dropping a photo you already added, with this turned on, now saves the copy too.",
      "Manage text recognition in one place under Settings → Storage: it sits with the other on-device components, where you install it or remove it (and its image libraries) to reclaim space, with clear confirmations. Removing it just means new photos are indexed by date and location only.",
    ],
  },
  {
    version: "2.41.0-alpha",
    date: "2026-06-28",
    highlights: [
      "Photos and screenshots can now be ingested (engine side): drop in a JPG/PNG/WebP/HEIC and PM reads the text inside it (when text recognition is enabled), pulls the capture date and location from the photo, and makes it searchable alongside your documents — including a clean “that screenshot from March” style of recall.",
      "Your originals stay where they are by default; nothing is copied unless you ask. (The drag-and-drop with the “save a copy into my vault” option arrives in the next update.)",
      "Photos rebuild from your vault like any document, and the recognized text is never re-run on a rebuild — so re-indexing stays fast.",
    ],
  },
  {
    version: "2.40.0-alpha",
    date: "2026-06-28",
    highlights: [
      "Groundwork for a new feature arriving shortly: dropping in photos and screenshots so the text inside them becomes searchable, on-device. This update lays the storage and text-recognition plumbing; the drag-and-drop and the “read the text in my screenshots” experience land in the next couple of updates.",
      "Text recognition (OCR) is an optional add-on you choose to install — it won't bloat PM if you never use it, and you'll be able to remove it again any time from Settings → Storage.",
    ],
  },
  {
    version: "2.39.1-alpha",
    date: "2026-06-28",
    highlights: [
      "Fixed a bug where a document with a very long unbroken run of text (a dense table, a code dump, a no-spaces blob) could fail to be indexed — the embedder choked on the over-long piece and skipped it silently. Such pieces are now sized correctly and, as a safety net, always trimmed to fit, so the whole document gets indexed.",
      "Because that changes how a few documents are broken into pieces, PM will re-index your library once on the next launch — no action needed; search just works once it finishes.",
      "Quieted a stream of harmless “FontBBox” warnings some PDFs printed to the log during import.",
    ],
  },
  {
    version: "2.39.0-alpha",
    date: "2026-06-28",
    highlights: [
      "Set a project's priority yourself. Triage now has a Priority picker (High / Medium / Low, or Auto for no tag) — the honest replacement for the old guessed tag. It shows on the project's card and you can sort by it.",
      "Sort your focus list. A new Sort control reorders your projects by Deadline, Priority, Size, or most-recent activity — and the ↑/↓ button flips the direction. “Smart” (the default) keeps the most pressing first, just as before; your choice is remembered.",
    ],
  },
  {
    version: "2.38.0-alpha",
    date: "2026-06-28",
    highlights: [
      "Chatting in a project or editing its milestones now counts as activity. A project's “active” date — and whether it's gone quiet enough to read “Take a look” — used to move only when its documents changed; now engaging with the project itself keeps it current.",
      "A project's priority is now something you set, not a guess. The old “high/medium” tag was inferred from your most important document in the project, which didn't really say whether the project mattered — so it's gone. You'll set priority yourself in Triage in the next update; until then a project simply shows no priority tag.",
    ],
  },
  {
    version: "2.37.0-alpha",
    date: "2026-06-28",
    highlights: [
      "Milestones got tidier. The name and date fields now fit whatever width you give them, so the side panel never has to scroll sideways — and the date box no longer gets clipped when you triage a project on the Focus view.",
      "Marking a milestone done is now obvious: tick its checkbox and it gets a “Done” tag with a line through it. The nearest one you still haven't ticked keeps driving the project's “Due soon” status, as before.",
      "You can resize a project's side panel — drag its left edge to make it wider or narrower. The width is kept in proportion as you resize the app and remembered for next time.",
    ],
  },
  {
    version: "2.36.0-alpha",
    date: "2026-06-28",
    highlights: [
      "Projects can now have more than one deadline. A project usually has several — a pitch, a presentation, an internal due date — so the single deadline box is replaced by a list of milestones, each with its own label and date. Add them when you triage a project on the Focus view, or in the project's own page. Your existing deadline is carried over as a milestone automatically.",
      "Your project's status stays a single, honest signal: “Due soon” now follows the nearest milestone you haven't ticked off yet — finish your pitch, tick it, and the next deadline quietly takes over.",
      "Link a milestone to a calendar event and its date keeps itself in sync with your calendar (which stays the source of truth) — handy for the deadlines that are really meetings. Tap 📅 next to a milestone's date to pick the event.",
    ],
  },
  {
    version: "2.35.0-alpha",
    date: "2026-06-28",
    highlights: [
      "More groundwork for project milestones: PM can now track several deadlines per project under the hood, and a project's focus status (“Due soon”, etc.) is worked out from whichever milestone is nearest and still unmet — so finishing your pitch automatically lets the next deadline take over. The list editor to add and tick off these milestones arrives in the next release; nothing changes in what you see today.",
    ],
  },
  {
    version: "2.34.0-alpha",
    date: "2026-06-28",
    highlights: [
      "Groundwork for project milestones: a project will soon be able to carry several dated deadlines — a pitch, a presentation, an internal due date — instead of just one. This release lays the storage for that; your current single deadline is carried over automatically, so nothing changes in how you use PM yet. The new milestone list arrives in an upcoming release.",
    ],
  },
  {
    version: "2.33.0-alpha",
    date: "2026-06-27",
    highlights: [
      "Shared (Team) drives are now indexed just once across your Google accounts. If two connected accounts can both see the same shared drive, PM no longer indexes it twice — whichever account syncs it first “owns” it, and that drive shows up greyed out (“Already synced by …”) when you expand your other accounts. Disconnect the owning account and another account with access automatically takes it over on its next sync; a shared drive's files only become “source unreachable” once no connected account can reach it.",
      "Upgrade note: your existing shared-drive items are rebuilt under the new shared index on the next sync (they're index-only pointers and summaries, so nothing real is lost). Press “Sync now” to refresh them.",
    ],
  },
  {
    version: "2.32.0-alpha",
    date: "2026-06-27",
    highlights: [
      "You can now connect a Google account that signs in with its own Cloud project, separate from the shared one. This is what makes Google Advanced Protection accounts work — Google blocks those from using any shared third-party project. Under the Drive or Calendar connect button, open “Advanced Protection account? Use its own project”, paste that project's Client ID + secret, and sign in; PM remembers it for that account and uses it for every later refresh.",
      "Because two Advanced-Protection accounts can't share a project, each can carry its own — so you can connect several of them, one project apiece. A project entered for an account's Drive also covers its Calendar (same Google account).",
    ],
  },
  {
    version: "2.31.0-alpha",
    date: "2026-06-27",
    highlights: [
      "Connecting a Google Drive or OneDrive account no longer starts indexing straight away. The account now lands ready-but-unsynced with its scope chooser already open, so you can pick what to index first — your whole drive, just certain folders, or (for Google) which shared drives — and then press “Sync now” to start. Nothing is fetched until you do.",
      "Choosing what to sync now saves as you go — the Save button is gone. Tick My Drive, switch a drive to “Choose folders”, or add a shared drive and it's stored immediately; it takes effect the next time you press “Sync now”.",
    ],
  },
  {
    version: "2.30.0-alpha",
    date: "2026-06-27",
    highlights: [
      "Multi-calendar is here. You can now connect several Google Calendar accounts — each shows under Google with its own list of calendars to sync.",
      "Outlook / Microsoft 365 calendars now connect with a read-only sign-in, right under Microsoft — pick which calendars to sync, just like Google.",
      "Apple iCloud calendars have their own spot under Apple — add one by its public iCal link (no Apple sign-in needed).",
      "Everything you connect — Google, Outlook, Apple, and any iCal link — flows into one read-only calendar that powers your agenda, “what's on tomorrow” in chat, and the “Due soon” status. Read-only for now; nothing is written back to any provider.",
    ],
  },
  {
    version: "2.29.0-alpha",
    date: "2026-06-27",
    highlights: [
      "Groundwork for multi-calendar support: behind the scenes, your calendar now records each connected account and individual calendar in a cleaner structure — the foundation for bringing several Google accounts, Outlook (Microsoft 365), and Apple calendars together in one place. How you use it doesn't change yet; the new controls arrive next.",
      "Each event now keeps its stable calendar ID, so a future release can link an event to the project or deadline it belongs to.",
      "Your existing Google Calendar connection carries over automatically — no need to reconnect.",
    ],
  },
  {
    version: "2.28.0-alpha",
    date: "2026-06-27",
    highlights: [
      "The Connectors tab got a visual tidy-up — far fewer nested boxes — so each provider's sign-in, accounts, and options are easier to follow at a glance.",
      "Indexing speed (Fast / Gentle) now tucks its explanation behind a short “What do Fast and Gentle do?” toggle, open by default only on Power density. The summary also makes clear it paces Drive and file indexing (email later) — calendars are tiny and always sync at full speed.",
      "The “Using more than one Google account?” help now links straight to the right Google Cloud page (Audience → Test users); the old link landed on a dead overview screen. It also explains that an app left in Testing mode signs accounts out after 7 days, and recommends publishing it to Production for a connection that lasts.",
      "The “first sync indexes your entire Drive” note now appears only while that first indexing is actually running — right by the progress bar — and tucks itself away when it finishes, reappearing when you add a new account. Same for OneDrive.",
      "You can now start a sync for another connected account while one is already indexing: its “Sync now” stays enabled and the account joins the queue (it shows “Queued” until its turn). Same for OneDrive.",
      "Google Calendar now shows which account you're signed in as, above the list of calendars to sync — handy when you have several Google accounts.",
      "The calendar-subscription guide now covers Outlook and Apple iCloud as well as Google, with step-by-step instructions for finding each one's private iCal/ICS link.",
    ],
  },
  {
    version: "2.27.0-alpha",
    date: "2026-06-27",
    highlights: [
      "Connecting a second Google (or Microsoft) account now works properly: when you click “Add another account”, the sign-in always shows the account chooser, so you can pick a different account instead of it silently re-using the one you're already signed in with.",
      "The Connectors tab is now grouped by provider — Google, Microsoft, Apple — instead of by Calendar / Drive / Email. Each provider's sign-in is set up once at the top of its group and shared across all of that provider's services (Google: Calendar + Drive; Microsoft: OneDrive), so a calendar-only user no longer has to hunt around the Drive section to find it.",
      "New “Using more than one Google account?” help sits right under the Google sign-in: reuse the one project, add each account under Test users, then pick it from the chooser — no second project needed.",
      "When you connect your first Google or Microsoft account, a short note reminds you that the account you connect first heads the list, so you can connect your main one first.",
      "Calendar subscriptions (iCal, no sign-in) now have their own clearly-labelled section, since they don't belong to any one provider.",
    ],
  },
  {
    version: "2.26.0-alpha",
    date: "2026-06-27",
    highlights: [
      "New connector: Microsoft OneDrive. Under Connectors → Drive, set it up once (a free Microsoft “Mobile & desktop” app registration — just paste the client ID, there’s no secret to copy), then connect one or more OneDrive accounts. Works with both personal Microsoft accounts and work/school accounts.",
      "Everything stays index-only, exactly like Google Drive: PM stores a searchable pointer and a short summary, never the file itself — the full file stays in OneDrive and is fetched on demand. The first sync indexes your whole OneDrive; later syncs only fetch what changed (via OneDrive’s delta query).",
      "Index your whole OneDrive, or expand an account and switch to “Choose folders” to index just the folders you pick (subfolders included). Files that fall out of scope stay findable but stop syncing new changes.",
      "Same robust syncing as Drive: it runs in the background (leave the page and come back), can be stopped (keeping everything indexed so far), resumes automatically after a crash, and shows a results summary — including any files it couldn’t read.",
      "Disconnecting an account keeps its indexed items findable (marked “source unreachable”) and never deletes them; reconnect to resume.",
    ],
  },
  {
    version: "2.25.0-alpha",
    date: "2026-06-27",
    highlights: [
      "Set a document's importance right from a project's Files panel — the same High / Medium / Low / Archive toggle as the Review tab, so triaging happens where you're already looking. On Power density the panel also gives you the full Review-style controls (re-file the project, edit tags); Standard keeps just the importance toggle.",
      "Those focus-panel controls follow your 'Review & Teach tabs' setting: if you've decided PM's auto-filing is good enough and hidden those tabs, the manual controls in Focus hide with them and the panel goes back to a clean read-only list.",
      "New Archive importance level (it replaces 'None'). Archiving shelves a document: it drops off the Map and sinks to the bottom of the Documents list, but stays fully searchable — including by exact keyword. Brand-new, un-triaged documents still show on the Map as before, so archiving is now a deliberate choice rather than just 'not set yet'.",
      "A project's Files panel can now be sorted by name or importance — click a heading to sort, click it again to reverse.",
      "The Review queue now orders itself by importance: the AI's most-important suggestions rise to the top, and it re-sorts live as proposals come in, so you triage what matters first.",
      "If Google Calendar sync fails only because the Calendar API isn't switched on in your Google Cloud project, PM now shows a one-click 'Enable the Google Calendar API' link (pointed at your project) instead of a raw error — connect, enable, sync.",
    ],
  },
  {
    version: "2.24.0-alpha",
    date: "2026-06-27",
    highlights: [
      "Connected a personal Google account? You can now index just the folders you choose from My Drive instead of the whole thing. Your whole drive stays the default — but under Connectors → Drive, switch My Drive to 'Choose folders' and pick the ones you want (subfolders included), the same way shared drives already work.",
      "Everything stays index-only (a pointer and summary, never the file bytes), and the change applies on your next Sync now: newly-in-scope files get indexed, and files that fall outside the folders you picked stay findable but stop syncing new changes.",
    ],
  },
  {
    version: "2.23.0-alpha",
    date: "2026-06-27",
    highlights: [
      "New Settings → Storage tab: see what PM has downloaded to this device — the document engine, the enhanced map layout, the speech model, and your search model — with sizes, and remove what you don't need to free space. Everything here re-downloads on demand if you want it back.",
      "Removals are safe by design: the heavy shared libraries behind the enhanced map layout can only be removed once nothing still uses them — a greyed Remove button points you to what to turn off first — and removing them asks an extra confirm that recommends keeping them. The libraries PM relies on elsewhere are never offered for removal.",
      "The enhanced (t-SNE) layout's Remove moved to the new Storage tab; its on/off switch stays under Settings → Memory map.",
      "The Map can now write each document's file name inside its node — turn 'Names' on in the Map's top bar (next to the arrangement toggle) to read nodes at a glance instead of hovering. The text is sized to the node and a long name is shortened to fit, so zoom into a bigger node to read more of it.",
    ],
  },
  {
    version: "2.22.1-alpha",
    date: "2026-06-27",
    highlights: [
      "Behind the scenes: routine maintenance updates to a few bundled dependencies, keeping things current. No change to the app itself.",
    ],
  },
  {
    version: "2.22.0-alpha",
    date: "2026-06-27",
    highlights: [
      "The Map can now arrange your documents by meaning: similar ones sit close together, worked out from their content on your device. Switch between Semantic and By-project in the Map header (or set a default under Settings → Memory map). It's computed quietly in the background and cached, so it's ready when you open it.",
      "Semantic proximity uses a fast built-in layout by default; for sharper, tighter clusters you can download an optional enhanced (t-SNE) component — a one-time download that then runs fully on your device — from the Map or Settings → Memory map.",
      "Settings → Memory map also lets you cap how many documents are plotted (200–5,000; default 1,000) so the Map stays comfortable on large libraries — beyond the cap, the rest are gathered at their project's spot rather than dropped.",
      "The Map is now fully navigable: scroll to zoom (scroll sideways to pan left/right), drag to pan, and double-click (or the Fit button) to reset — in every arrangement. Large project clusters (e.g. right after a big import) now pack together more tightly instead of spreading out.",
      "An optional Project cohesion control (Off by default) gently pulls same-project documents together in the semantic view, if you'd like projects to read as looser clusters while meaning still drives the layout — it's in the Map's top bar and Settings → Memory map.",
      "Downloading the enhanced (t-SNE) layout shows a progress bar, and once it's installed you can switch it on or off — or remove it to free space — from Settings → Memory map.",
    ],
  },
  {
    version: "2.21.0-alpha",
    date: "2026-06-27",
    highlights: [
      "The Map is faster and smoother with large libraries: it's now drawn on a single canvas instead of one shape per node, so panning and hovering stay responsive even with thousands of documents (a big Drive sync adds a lot). It also loads quietly in the background when the app starts — off to the side so it never stutters the rest of the app — so opening the Map tab is instant.",
      "Click any document node on the Map to jump straight to its project. A dashed ring now marks documents still awaiting review or held index-only (in a connected source like Drive, not stored on this device); hover a node — with help mode on — to see what its size and colour mean.",
    ],
  },
  {
    version: "2.20.1-alpha",
    date: "2026-06-27",
    highlights: [
      "Behind the scenes: PM's design-system primitives — buttons, cards, list rows, badges, dialogs and the rest — are now mirrored to Claude Design, so on-brand mockups can be built from the app's real components. No change to the app itself.",
    ],
  },
  {
    version: "2.20.0-alpha",
    date: "2026-06-27",
    highlights: [
      "The Indexing speed setting (Fast / Gentle) moved to the top of Settings → Connectors — that's where it matters most, since a big Drive sync is the longest index. Switching it now takes effect immediately, even partway through a sync; before, a change only applied to the next run.",
      "Gentle indexing is now easier on memory, not just the processor: it embeds in smaller batches, so a low-memory machine stays usable while a large index runs. Each mode is spelled out — Fast uses as much CPU and memory as it needs; Gentle paces the work and uses less of both.",
      "Settings → Usage & cost no longer reads blank for a model you've used both before and after the real-cost update. It now adds the real per-call cost OpenRouter reports and only estimates the older calls that predate it — so your real spend always shows, instead of one older call hiding the whole model's cost.",
    ],
  },
  {
    version: "2.19.0-alpha",
    date: "2026-06-26",
    highlights: [
      "Google Drive now indexes shared drives (Team Drives), not just your personal My Drive. Expand a connected account under Settings → Connectors → Drive to pick which shared drives to add. Shared drives are folder-scoped by default — choose the folders you want (everything inside is indexed) or switch a drive to index entirely. Still index-only: the files stay in Drive.",
      "The Connectors sections (Calendar, Drive, Email) are now collapsible so the tab stays tidy — expanded by default on Standard and Power density, collapsed by default on Minimal, and you can fold either way.",
      "External links throughout the app — the Google setup guide, OpenRouter, release notes — now open in your default browser when clicked (previously they did nothing in the app window).",
      "Drive indexing now runs in the background: start a sync and you can leave the Settings page — it keeps working, and the progress reappears when you come back. One “Sync now” per account covers My Drive and your chosen shared drives.",
      "You can Stop a Drive sync mid-index if it's bigger than you expected — everything indexed so far is kept and stays searchable; only the rest is left for next time. You can also add another Google account while one is still indexing, and if PM is closed or crashes during a large index, it picks up where it left off on the next launch (already-indexed files are never lost).",
      "After a sync, Drive shows a summary: how many files were indexed, updated, or removed, plus an expandable list of any it couldn't read (e.g. an unsupported file type) with the reason — so nothing is silently dropped.",
      "The Review tab no longer re-runs (and re-bills) its AI suggestions every time you leave and come back — proposals are remembered until you commit them or press Re-propose. Helpful while a large Drive index is filling the queue and you want to peek at other tabs.",
      "Settings → Usage & cost now shows the real spend OpenRouter reports for each call — including prompt-cache savings — instead of a local estimate that could read blank when a model wasn't in the cached price list. (Costs from before this update still use the estimate.)",
      "Cheaper sorting: when the Review tab proposes filing for a batch of documents it now reuses a cached prompt prefix across the run, so models that support prompt caching bill the shared instructions once instead of per document — noticeably less for a large import.",
      "Wide tables (the developer Raw-table browser and retrieval explainer) now pan sideways with a plain mouse wheel; the Documents list is sortable by any visible column; and the Review cards explain — in help mode — what Project, Importance, and Tags each mean.",
      "New Indexing speed setting (Settings → Connectors): Gentle paces indexing so a low-end computer stays usable while a large Drive sync or import runs in the background; Fast (default) indexes at full speed.",
    ],
  },
  {
    version: "2.18.0-alpha",
    date: "2026-06-26",
    highlights: [
      "Google Drive sync (Settings → Connectors → Drive): connect your Google account and PM indexes your Drive so you can search it and ground answers in it. It's index-only — PM keeps a searchable pointer and a short summary, never a copy; the file stays in Drive and is fetched on demand. Connect more than one account; each syncs on its own.",
      "The first sync walks your whole Drive (it warns you it can take a while); after that only changes are fetched. Files removed in Drive stay findable but are flagged 'source missing' rather than dropped. From Documents you can open an indexed file in Drive or pull its full text on demand.",
    ],
  },
  {
    version: "2.17.0-alpha",
    date: "2026-06-26",
    highlights: [
      "New Connectors tab in Settings, grouped by what each account does — Calendar, Drive, and email. The read-only calendar moved here from its own tab: connect Google Calendar with your own sign-in, or paste a no-sign-in calendar subscription (iCal) link.",
      "Each connection is independently opt-in and removable, and your Google sign-in is set up once and shared across Google services. Google Drive (index-only) arrives next; Microsoft and Apple show as coming soon.",
    ],
  },
  {
    version: "2.16.0-alpha",
    date: "2026-06-26",
    highlights: [
      "Developer mode gains a “Retrieval explain” tool (Dev tab): type a query and see exactly why the assistant's search ranks the chunks it does — vector distance, keyword rank, fused score, recency decay, and reranker score — side by side. Handy for confirming the index returns the right material and seeing what reranking changes.",
      "It runs the same retriever chat uses but changes nothing: it's read-only, chunk text is shown only as a short truncated preview, and no document bodies or secrets are surfaced.",
    ],
  },
  {
    version: "2.15.0-alpha",
    date: "2026-06-26",
    highlights: [
      "Developer mode now reveals internals right where each feature lives (turn it on under Settings → Developer): the Teach tab shows each project's and preference's raw id, confidence, and confirmed state; Documents lets you click a chunk count to inspect that file's chunk breakdown; and Calendar shows a read-only sync-state read-out.",
      "Same safety rules as the Dev tab — everything is read-only and personal or large fields (chat text, document bodies) and any secrets are never shown.",
    ],
  },
  {
    version: "2.14.0-alpha",
    date: "2026-06-26",
    highlights: [
      "New Developer mode for the technically curious: a plainly-labelled switch under Settings → Developer adds a read-only “Dev” tab that shows PM's internals — system & build info, row counts across the store, a raw table browser, and your corrections log. It's strictly read-only (it never changes your data) and independent of the density preset.",
      "Built so it's safe to share a screen: personal and large fields (chat text, document bodies) are truncated or shown only as a length, and secrets are never surfaced. More features will add their own read-only diagnostics here over time.",
    ],
  },
  {
    version: "2.13.0-alpha",
    date: "2026-06-26",
    highlights: [
      "The Teach tab now has a Preferences section where you can see and shape how PM works for you. Add a preference in plain words (“keep replies short during work hours”) and PM fills in the details, or set it out yourself — scoped to everywhere, one project, or a situation.",
      "Preferences carried over from your old “Learning You” profile show up here marked “Suggested”, ready for you to keep, edit, or remove — so PM only acts on the ones you've vouched for. The old read-only profile box in Settings is gone; it now points you here.",
    ],
  },
  {
    version: "2.12.0-alpha",
    date: "2026-06-26",
    highlights: [
      "PM now keeps a structured memory of how you like things done. Instead of one ever-growing note, your preferences are kept as separate rules — each tied to where it applies (everywhere, one project, or a particular situation) — so chat, sorting suggestions, and your daily briefing draw on just the ones that fit the moment, rather than everything at once.",
      "Your existing “Learning You” profile is carried over into these structured preferences automatically, so nothing you've taught PM is lost. Viewing and editing them lands in the Teach tab in the next update.",
    ],
  },
  {
    version: "2.11.0-alpha",
    date: "2026-06-26",
    highlights: [
      "Projects you've deliberately set up — by renaming, merging, adding a name, or correcting one in Review — now show a “✓ Confirmed” mark in the Teach tab, so you can tell at a glance which ones you've vouched for versus which were filed automatically.",
      "That confirmed status travels with your vault: it's saved alongside your project names in the same encrypted rules file, so copying your vault to another device — or rebuilding the search index — keeps it intact.",
    ],
  },
  {
    version: "2.10.0-alpha",
    date: "2026-06-26",
    highlights: [
      'Index-only items now react to changes at their source: an edit re-indexes automatically, and a file that\'s deleted or a source that goes offline keeps the item findable with a clear badge ("Source missing" / "Source unreachable") instead of silently vanishing. This is the second half of the index-only foundation; the connectors that detect those changes come next.',
      "A renamed or moved item keeps its place — its project, tags, and search position ride a stable id, so reorganising a folder at the source never wipes how you'd filed it.",
    ],
  },
  {
    version: "2.9.0-alpha",
    date: "2026-06-26",
    highlights: [
      'Groundwork for a new "index-only" mode: PM will be able to make cloud files and watched folders searchable by keeping a short summary and a pointer to the original, instead of importing a full copy. This release lands the storage + search foundation; the connectors that add real sources come next.',
      'Documents that work this way are clearly badged "Indexed-only" — they stay findable and their summary reads offline, while the full document is fetched from its source when you open it.',
    ],
  },
  {
    version: "2.8.0-alpha",
    date: "2026-06-25",
    highlights: [
      "Setting up the document engine, importing files, and re-indexing your library now show real progress instead of just a busy shimmer — so you can see how far along things are and roughly how much is left.",
      "It follows your Depth setting: Minimal keeps the calm shimmer, Standard adds a filling bar with an 'X of Y' document count, and Power adds a live percentage. The one-time engine setup and model download still shimmer, since there's no total to count there.",
    ],
  },
  {
    version: "2.7.1-alpha",
    date: "2026-06-25",
    highlights: [
      "Search quality fix: PM had been breaking your documents into far more — and far smaller — fragments than intended, which watered down both document search and chat answers. Each passage is back to the right size, so results are more coherent and to the point.",
      "Because this changes how your library is indexed, PM will suggest a one-time Rebuild for existing vaults. It re-reads your own Markdown files and changes nothing in your documents or notes.",
    ],
  },
  {
    version: "2.7.0-alpha",
    date: "2026-06-25",
    highlights: [
      "Settings is tidier. Everything now lives in five tabs down the side — General, AI & Models, Search, Calendar, and Data & Security — so related options sit together instead of in one long scroll. Nothing was taken away; it's just easier to find.",
      "Settings also follows your Depth setting now. On the Power layout each section opens with its full detail (your spend breakdown, model recommendations, the profile PM has learned about you); on Standard and Minimal that detail starts neatly collapsed and is one click away — so the page stays calm without ever hiding a setting. The first-run welcome is unchanged.",
    ],
  },
  {
    version: "2.6.1-alpha",
    date: "2026-06-25",
    highlights: [
      "Fixed a Windows issue where the document engine could fail to set up the first time you used it — the bundled Python was packaged with its folders flattened, so it couldn't start and showed a confusing error. A clean install now sets up correctly.",
      "If document-engine setup ever fails like that again, PM now recognises it as a PM packaging problem (not something wrong with your computer) and gives you a one-click, pre-filled way to report it.",
    ],
  },
  {
    version: "2.6.0-alpha",
    date: "2026-06-25",
    highlights: [
      "Meet Teach — a new tab for tidying how your projects are named. If the same project ever shows up under two names, merge one into the other in a click and the duplicate stops coming back; rename a project and it updates on every document at once; or add a name you know means the same thing. PM even flags obvious look-alikes for you. It does the same thing as correcting a project in Review — just somewhere you can do it deliberately.",
      "Teach is on for the Standard and Power layouts and tucked away under Minimal — show or hide it anytime under Settings → Appearance. Hiding it only hides the tab; the naming rules you've already taught PM keep working underneath.",
    ],
  },
  {
    version: "2.5.0-alpha",
    date: "2026-06-25",
    highlights: [
      "Corrections stick now. When you fix which project a document belongs to in the sorting review, PM remembers it as a lasting rule rather than a one-off edit — so if it had been proposing the same project under slightly different names (say “PM”, “Personal Manager” and “Atlas - PM”), correcting it once teaches PM they’re the same, and that variant stops reappearing every time you sort new files.",
      "Behind the scenes your projects now have a stable identity, kept in a small encrypted file alongside your library so the rules travel with your vault and survive a re-index. Nothing in your documents or notes is changed — this just records how you like things organised. A dedicated place to rename and merge project names is coming next.",
    ],
  },
  {
    version: "2.4.0-alpha",
    date: "2026-06-25",
    highlights: [
      "Switch your search language anytime — now for any vault, not just new ones. Under Settings → Search you can move a vault between English and Multilingual whenever you like. Multilingual understands 100+ languages and finds meaning across them (not just matching words), so it's the one to pick if your library isn't only in English; the first time you turn it on it downloads a larger language model (about 1 GB, once). Switching re-indexes your library from your own Markdown files — your documents are never changed, and if you're offline it simply leaves things as they were until you're back online.",
      "Good to know if you ever go back: a vault you've switched to Multilingual is built for this version, so opening it in an older copy of PM will make search misbehave until you update again. Your files and notes are always safe either way.",
    ],
  },
  {
    version: "2.3.0-alpha",
    date: "2026-06-25",
    highlights: [
      "Sharper search results: PM now re-ranks what it finds with a second, more precise pass, so chat answers and document search surface the most relevant passages first. It's on by default — the first search downloads a small model — and there's a new “Re-rank search results” switch under Settings → Search if you ever want the fastest, lightest results instead.",
      "Multilingual search for new vaults: when you set up a fresh vault you can now choose a Multilingual search language (great if your notes aren't only in English) instead of the default English. Existing vaults keep working unchanged; switching an existing vault's language (which re-indexes it) is coming in a later update.",
    ],
  },
  {
    version: "2.2.0-alpha",
    date: "2026-06-25",
    highlights: [
      "Better search foundation: PM now splits your documents into smarter, structure-aware pieces — it keeps headings, code blocks, and tables intact and carries each section's heading into what gets searched — so answers draw on more relevant passages. The search models are now swappable behind the scenes too, setting up multilingual support next. After updating, the Documents view offers a one-time “Rebuild” to re-index your existing files with the improvement; your documents and notes aren't changed, just re-indexed.",
    ],
  },
  {
    version: "2.1.5-alpha",
    date: "2026-06-24",
    highlights: [
      "Linking a second Windows account to a shared vault is clearer: Settings → Vault now shows you exactly what to enter (an account name or SID, not a folder path), the PowerShell command to look it up, and the difference between the two — with the stable SID recommended.",
    ],
  },
  {
    version: "2.1.4-alpha",
    date: "2026-06-24",
    highlights: [
      "Document engine setup is more reliable on macOS: PM now finds a modern Python automatically (including Homebrew and python.org installs) and rebuilds its engine after a Python upgrade — so setup just works, with no Terminal steps. If it still can't finish, a new troubleshooting popup explains exactly what to do.",
    ],
  },
  {
    version: "2.1.3-alpha",
    date: "2026-06-24",
    highlights: [
      "Documentation refresh: the README is now up to date with everything PM does today and written to stay current as features land, and the project ships public Contributing and Releasing guides covering how changes are reviewed, gated, and shipped. No change to the app or your data.",
    ],
  },
  {
    version: "2.1.2-alpha",
    release: true,
    date: "2026-06-24",
    highlights: [
      "Clearer downloads: the release page now leads with a simple, one-file install guide for Windows and macOS — it's obvious which single file to grab and how to open it. No change to how you use PM.",
    ],
  },
  {
    version: "2.1.0-alpha",
    date: "2026-06-24",
    highlights: [
      "Shared & portable vaults: you can now protect your vault with a passphrase instead of tying it to this one Windows account — so the same vault can be opened from another profile on this machine. Choose it during setup, or switch anytime in Settings → Vault.",
      "Your files stay yours. A passphrase-protected vault keeps its Markdown encrypted at rest, and you can export everything to plain Markdown at any time with your passphrase — encryption protects your notes, it never locks you in.",
      "Safe to share: only one profile writes at a time. If PM is already open in another profile, you'll get a calm “Continue here?” hand-off instead of two copies racing over your data.",
      "Zero-friction by default: if you don't opt in, your vault stays private to this device exactly as before, with its key held in your OS keychain — nothing new to remember.",
    ],
  },
  {
    version: "2.0.1-alpha",
    release: true,
    date: "2026-06-23",
    highlights: [
      "Behind-the-scenes fixes to how PM is packaged and shipped, so the Windows installer builds correctly and the macOS app bundles cleanly — no change to how you use PM. (This is the build that delivers the first public release.)",
    ],
  },
  {
    version: "2.0.0-alpha",
    date: "2026-06-23",
    highlights: [
      "Welcome to the first public release of PM — your private, local-first personal manager that keeps your documents, your focus, and the moving parts of your day in one calm place.",
      "Everything stays on your machine. Your encrypted store stays encrypted, and PM never sends your data anywhere on its own.",
      "A polished, fully themeable interface — light and dark, accent colours, and depth — plus an optional app lock so a quick glance can't open your things (Windows Hello).",
      "On Windows, PM is completely self-contained: there's nothing extra to install, and from here on it quietly keeps itself up to date.",
    ],
  },
  {
    version: "1.3.5-alpha",
    date: "2026-06-23",
    highlights: [
      "Security hardening (nothing changes in how you use PM): your sensitive values — the database key, your OpenRouter keys, and your Google Calendar tokens — are now held in a wrapper that can never be accidentally written to a log or error message, so they stay protected even as the app keeps growing.",
    ],
  },
  {
    version: "1.3.4-alpha",
    date: "2026-06-23",
    highlights: [
      "Clearer wording about what’s protected at rest: your encrypted store stays encrypted, but the documents in your Markdown vault are stored in the open — so Settings → Data now reminds you to turn on full-disk encryption (BitLocker / FileVault) to protect them.",
    ],
  },
  {
    version: "1.3.3-alpha",
    date: "2026-06-23",
    highlights: [
      "Developer tooling only (no change to the app or your data): PM’s code-quality checks now skip the bundled Python runtime on Windows, so local builds stay clean.",
    ],
  },
  {
    version: "1.3.2-alpha",
    date: "2026-06-23",
    highlights: [
      "The “Day-to-day” model suggestion now skips free models — they’re often rate-limited and unreliable, which caused chat to silently fall back to your second-choice model. It now recommends the cheapest dependable model instead.",
    ],
  },
  {
    version: "1.3.1-alpha",
    date: "2026-06-23",
    highlights: [
      "Fixed the Focus tab getting stuck on its loading placeholder instead of showing your projects.",
      "Focus now opens instantly when you switch back to it from another tab, instead of reloading each time.",
    ],
  },
  {
    version: "1.3.0-alpha",
    date: "2026-06-22",
    highlights: [
      "On Windows, PM is now fully self-contained — you no longer need to install Python yourself before using the document features. Everything PM needs to run ships inside the app.",
      "The document engine still does its one-time setup the first time you add a document (it downloads the tools and models it needs); after that it's ready and offline-capable.",
    ],
  },
  {
    version: "1.2.1-alpha",
    date: "2026-06-22",
    highlights: [
      "Added a security policy (SECURITY.md): how to privately report a security issue, which versions get fixes, and what's in and out of scope. No change to your data or how you use PM.",
    ],
  },
  {
    version: "1.2.0-alpha",
    date: "2026-06-22",
    highlights: [
      "Behind-the-scenes groundwork: PM now runs an automated quality and safety net on every change — formatting, type, security, dependency, and version checks — so updates stay consistent and dependable. No change to your data or how you use PM.",
    ],
  },
  {
    version: "1.1.2-alpha",
    date: "2026-06-22",
    highlights: [
      "Your data now lives in one clearly-named folder — “Personal Manager” — that's easier to find and back up, in the right machine-local spot for your OS.",
      "Your appearance settings and your Pinboard now travel with your data: they're kept inside your encrypted store rather than the browser layer, so backing up or moving your data folder carries them along.",
      "New in Settings → Data: “Open data folder” reveals everything in your file manager, and “Export all data” bundles your documents and store into a single .zip you choose where to save (the store stays encrypted in the archive).",
    ],
  },
  {
    version: "1.1.1-alpha",
    date: "2026-06-22",
    highlights: [
      "Polish: the document Map no longer flashes “No documents yet” while it's loading — it shows a gentle placeholder until your map is ready.",
    ],
  },
  {
    version: "1.1.0-alpha",
    date: "2026-06-22",
    highlights: [
      "Optional app lock: you can now ask PM to check it's you — with Windows Hello (your face, fingerprint, or PIN) — before it opens. Turn it on under Settings → App lock; it stays off until you do, and it only appears where your device can actually perform the check.",
      "It's a convenience lock for the window, not a second password on your data — your store is always encrypted at rest regardless. If your device can't verify for some reason, you can still get in, so you're never locked out of your own app.",
    ],
  },
  {
    version: "1.0.0-alpha",
    date: "2026-06-21",
    highlights: [
      "A whole new look. PM has been rebuilt on a proper design system, so every screen now shares one calm, consistent visual language — the same features you already use, freshly dressed.",
      "Make it yours: a new Appearance section in Settings lets you switch between three visual styles, light or dark mode, an accent colour, and a density from minimal to power-user — and PM remembers your choice on this device.",
      "A custom window frame replaces the stock title bar for a more polished, app-like feel, and dates now read consistently as day-month.",
      "Dates now follow your time zone. PM detects your device's zone automatically — or you can pin one in Settings — so “today”, what's “due soon”, and your calendar agenda all land on the right local day.",
      "Cleaner interactions: long results and source lists collapse into tidy cards you can expand, clicking a citation in an answer highlights the source it came from, and the actions you can't undo — like rebuilding the index or disconnecting a calendar — now ask you to confirm first.",
      "Smoother waits and more room: imports and indexing show a real progress bar, screens fill in with placeholder shapes instead of a blank pause while they load, and you can go full-screen with F11 or the title-bar button.",
      "See what you're spending: a new Usage & cost panel in Settings tracks your model costs — priced from OpenRouter's public rates, refreshed daily — with 30-day and all-time totals, a per-model breakdown, and a nudge toward a cheaper equivalent model when one would do.",
      "Help choosing a model: PM now suggests two — a low-cost, reliable Day-to-day pick and a higher-powered Advanced one for high-stakes chat — each with a short reason and its real price, ready to apply in a click. And every request now enforces zero-data-retention, so providers can't keep or train on your prompts.",
      "New Pinboard: a bounded board for thinking things through — drag, snap, and resize sticky notes and simple timelines. It's a private scratch space; nothing on it is filed or searched.",
      "If an automatic update ever can't install itself, PM now offers a direct download link instead of getting stuck — so you can always reach the latest version.",
    ],
  },
  {
    version: "0.8.6-alpha",
    date: "2026-06-20",
    highlights: [
      "Housekeeping ahead of the move to a single public home — the docs now live in one place alongside the code and releases. No change to your data or how you use PM.",
    ],
  },
  {
    version: "0.8.5-alpha",
    date: "2026-06-20",
    highlights: [
      "Reliability pass from a full code review: switching chats while a reply is still streaming no longer shows that reply under the wrong conversation, and confirming a sorting review is now all-or-nothing — an interrupted confirm can’t leave some documents half-filed.",
      "Sturdier importing and calendar handling — a huge or unusual document, a long voice clip, or an oddly-scheduled calendar event can no longer balloon memory or stall the app — plus clearer messaging when your store is briefly locked (e.g. by antivirus) instead of it looking like corruption.",
      "Under-the-hood tidying ahead of going open-source. No change to your data or how you use PM.",
    ],
  },
  {
    version: "0.8.4-alpha",
    date: "2026-06-20",
    highlights: [
      "PM now wears an “Alpha” label — in the app, in its window title, and in its version number — to make its stage clear as it heads toward a public release. It’s feature-complete for v1 and usable day to day, but still under active development, so expect the occasional rough edge.",
      "No change to your data or how you use PM.",
    ],
  },
  {
    version: "0.8.3",
    date: "2026-06-20",
    highlights: [
      "Defence-in-depth hardening ahead of going open-source: sensible limits everywhere so a hostile file, calendar feed, or huge pasted message can’t hog memory or run up your model bill, and PM now asks your system for only the permissions it actually uses.",
      "Your encryption key is now wiped from memory right after the store is unlocked, the store’s encryption settings are pinned so future updates can always reopen your data, and each update step is all-or-nothing so an interrupted update can’t leave it half-changed.",
      "Housekeeping for the move to a public home — no change to how you use PM.",
    ],
  },
  {
    version: "0.8.2",
    date: "2026-06-20",
    highlights: [
      "Security hardening from a full security review: calendar feeds now must use a secure https link and can’t point at a private or local address — so a feed link can’t be turned into a way to probe your own network, and it’s never sent in the clear.",
      "Plus under-the-hood robustness — a large or malformed document can no longer balloon memory while it’s being imported — with no change to how you use PM.",
    ],
  },
  {
    version: "0.8.1",
    date: "2026-06-19",
    highlights: [
      "Reliability polish: replies that include emoji, accents, or other languages now come through cleanly as they stream; the microphone always switches off the moment you stop recording or leave the chat; and your edits in Review are no longer overwritten by an AI suggestion that lands a split second later.",
      "A downloaded update now stays one click away after you choose “Later”, chat answers come back a little quicker, and a calendar that fails to sync no longer pretends it succeeded — plus a batch of smaller fixes from a full code review.",
    ],
  },
  {
    version: "0.8.0",
    date: "2026-06-19",
    highlights: [
      "Voice input: there’s now a microphone button in the chat box — click it, speak, and PM turns your words into text you can edit before sending. It works in both your main chat and a project’s chat.",
      "Private by design: your voice is transcribed entirely on your device — no audio ever leaves it. The first time you use it, PM downloads a small speech model once (about 145 MB), then it works fully offline.",
    ],
  },
  {
    version: "0.7.0",
    date: "2026-06-19",
    highlights: [
      "Daily briefing: your Focus screen now opens with a short “here’s your picture today” summary — what’s due soon, quick wins you could knock out, anything that’s gone quiet, and what’s on your calendar — written for you and grounded in your real projects.",
      "It refreshes itself about once a day when you open Focus, and there’s a Refresh button to rebuild it from your current state any time. It learns your voice from the same profile that powers PM’s suggestions.",
      "Everything stays on your device — the briefing is built from data PM already has, and only the model call leaves.",
    ],
  },
  {
    version: "0.6.0",
    date: "2026-06-19",
    highlights: [
      "Calendars (read-only): connect a calendar to see an Upcoming agenda on your Focus screen and ask chat things like “what’s on this afternoon?”. Everything stays on your device — only the fetch to your calendar leaves.",
      "Two easy ways to connect: paste a calendar’s private iCal feed URL (simplest — no sign-in, and it works even with Google Advanced Protection), or use full Google sign-in with your own credentials under Settings → Advanced.",
      "Due soon, automatically: when an upcoming event’s title names one of your projects, that project flips to “Due soon” and shows the event — no need to set a deadline by hand.",
      "You stay in control: feed URLs and any tokens live only in your OS keychain, and removing a feed or disconnecting wipes its synced events.",
    ],
  },
  {
    version: "0.5.0",
    date: "2026-06-19",
    highlights: [
      "Command palette: press Ctrl+K (⌘K on a Mac) — or click Search in the sidebar — to jump straight to any project, file, or past conversation. Just start typing.",
      "Pick a file and PM opens its project with that file highlighted; pick a project, a chat, or a 'Go to' destination and it takes you there — no clicking through the sidebar.",
    ],
  },
  {
    version: "0.4.0",
    date: "2026-06-18",
    highlights: [
      "Focus view: your new home screen shows every project on one page, each with a single honest status — Due soon, Quick win, Take a look, Blocked, Part of, or On track — so you can pick the one right thing to look at.",
      "Triage your projects: give a project a size, a deadline, a blocker, or a parent — by hand, or let the AI suggest them and just confirm. The status updates to match.",
      "Per-project view: click a project to narrow everything to just it — its files alongside a chat that answers only from that project's documents.",
    ],
  },
  {
    version: "0.3.0",
    date: "2026-06-18",
    highlights: [
      "Pick any model: the model picker now lists every model on OpenRouter — search by name, see each one's input/output price, sort by price, and read at-a-glance tags (Free, Reasoning, Vision, Coding, Long context…).",
      "Separate chat and background models: choose one model for your chats and a different one for behind-the-scenes work (sorting and learning) — handy for pairing a strong chat model with cheap or free background models.",
      "Auto-switch on limits: give each a ranked list of models and PM falls through to the next when one hits its daily limit, so you're never stuck mid-task.",
      "A models tag in the sidebar shows which chat and background models you're using right now, without opening Settings.",
      "Organise & Review: PM proposes a project, tags, and an importance level for each new document; your corrections are remembered.",
      "Learning You: a short, readable profile distilled from those corrections, fed back into PM's suggestions and chat so it gets more like you.",
      "Document map: a visual graph of your documents grouped by project, plus a Help mode that explains any part of the app on hover.",
    ],
  },
  {
    version: "0.2.0",
    date: "2026-06-17",
    highlights: [
      "Automatic updates: PM now updates itself in the background and offers a one-click restart when a new version is ready — no manual reinstalls.",
      "This “What's New” view, so you can always see what changed since your last version.",
    ],
  },
  {
    version: "0.1.0",
    date: "2026-06-15",
    highlights: [
      "Encrypted local store and streaming chat through OpenRouter.",
      "The Archivist: drag files or folders into Documents to convert, embed, and index them locally, with a rebuildable Markdown vault as the source of truth.",
    ],
  },
];
