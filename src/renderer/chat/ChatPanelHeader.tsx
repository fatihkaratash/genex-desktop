import type { JSX } from "react";
import { ChatHeader } from "../panels/ChatHeader.tsx";
import { ExportSheet } from "../panels/ExportSheet.tsx";
import type { ChatExport } from "./use-chat-export.ts";
import { useGameHistory } from "./use-game-history.ts";
import type { ChatParts } from "./use-chat-panel.ts";

/**
 * The chat's header, keyed by thread: its title (a game chat renames its game), export (a phone
 * opens the export sheet), reveal, its game's history space and compaction.
 */
export function ChatPanelHeader({
  props,
  chat,
  composer,
  chatExport,
}: ChatParts & { chatExport: ChatExport }): JSX.Element {
  const { sheet } = chatExport;
  return (
    <>
      <ChatHeaderBar props={props} chat={chat} composer={composer} chatExport={chatExport} />
      {sheet && (
        <ExportSheet
          project={sheet.project}
          destination={sheet.destination}
          onClose={chatExport.closeSheet}
          onExported={chatExport.sheetExported}
        />
      )}
    </>
  );
}

/** The header itself: title, export menu, reveal, history space and compaction. */
function ChatHeaderBar({
  props,
  chat,
  composer,
  chatExport,
}: Pick<ChatParts, "props" | "chat" | "composer"> & { chatExport: ChatExport }): JSX.Element {
  const { project, threadId } = chat;
  const history = useGameHistory(project, props.onNotice);
  return (
    <ChatHeader
      key={threadId}
      sidebarHidden={props.sidebarHidden}
      onToggleSidebar={props.onToggleSidebar}
      exporting={chatExport.exporting}
      onExport={project ? chatExport.exportGame(project) : undefined}
      history={project ? history : undefined}
      chatTitle={chat.chatTitle}
      isStudio={chat.isStudioThread}
      isDraft={chat.isDraft}
      project={project}
      pathLabel={chat.folder?.pathLabel}
      contextUsage={null}
      compacting={composer.compact.compacting}
      onCompact={composer.compact.compactNow}
      onRename={(title) => {
        if (project && props.onRenameGame) props.onRenameGame(project, title);
        else if (threadId) props.onRename(threadId, title);
      }}
      onReveal={project ? () => void window.studio.revealProject(project) : undefined}
    />
  );
}
