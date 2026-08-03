export const REDESIGN_ROUTE_CONTRACT = {
  coldLaunch: "/new/$draftId",
  newNote: "/new/$draftId",
  note: "/notes/$noteId",
  settings: "/settings",
} as const;

export const REDESIGN_ROUTE_LIMITS = {
  draftIdCharacters: 36,
  noteIdCharacters: 36,
  passageIdCharacters: 256,
  revisionIdCharacters: 64,
} as const;

export const REDESIGN_COMMAND_CONTRACT = {
  newNote: {
    id: "new-note",
    macosAccelerator: "CommandOrControl+N",
  },
  search: {
    id: "search",
    macosAccelerator: "CommandOrControl+K",
  },
  toggleSidebar: {
    id: "toggle-sidebar",
    macosAccelerator: "CommandOrControl+/",
  },
  settings: {
    id: "settings",
    macosAccelerator: "CommandOrControl+,",
  },
} as const;

export const REDESIGN_NAVIGATION_CONTRACT = {
  coldLaunch: "replace",
  firstCheckpoint: "replace",
  newNote: "push",
  searchSelection: "push",
  settings: "push",
  autosave: "none",
  checkpoint: "none",
} as const;

export type RedesignCommand = keyof typeof REDESIGN_COMMAND_CONTRACT;
export type RedesignNavigationAction = keyof typeof REDESIGN_NAVIGATION_CONTRACT;
