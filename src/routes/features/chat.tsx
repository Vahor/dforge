import { createFileRoute } from '@tanstack/react-router'
import { ChatEvent, events } from '../../commands'
import { getCurrent } from "@tauri-apps/api/webviewWindow";
import { useEffect, useRef, useState } from 'react';

export const Route = createFileRoute('/features/chat')({
  component: ChatComponent,
})

function ChatComponent() {

  const [chatMessages, setChatMessages] = useState<ChatEvent[]>([])
  const mounted = useRef(false);


  useEffect(() => {
    if (mounted.current) return;
    mounted.current = true;
    const window = getCurrent();

    console.log('Chat component loaded', window)
    events.chatEvent(window).listen((event) => {
      setChatMessages((prev) => [...prev, event.payload])
    })
  }, [])

  return (
    <div>
      {chatMessages.map((message) => {
        const id = `${message.timestamp}-${message.sender_name}`
        return (
          <div key={id}>
            {message.sender_name}: {message.content}
          </div>
        );
      })
      }
    </div >
  )
}