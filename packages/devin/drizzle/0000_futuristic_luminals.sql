CREATE TABLE `message_nodes` (
	`row_id` integer PRIMARY KEY AUTOINCREMENT NOT NULL,
	`session_id` text NOT NULL,
	`node_id` integer NOT NULL,
	`parent_node_id` integer,
	`chat_message` text NOT NULL,
	`created_at` integer NOT NULL,
	`metadata` text,
	FOREIGN KEY (`session_id`) REFERENCES `sessions`(`id`) ON UPDATE no action ON DELETE no action
);
--> statement-breakpoint
CREATE TABLE `prompt_history` (
	`id` integer PRIMARY KEY AUTOINCREMENT NOT NULL,
	`content` text NOT NULL,
	`timestamp` integer NOT NULL,
	`session_id` text NOT NULL,
	`is_shell` integer DEFAULT 0 NOT NULL,
	FOREIGN KEY (`session_id`) REFERENCES `sessions`(`id`) ON UPDATE no action ON DELETE no action
);
--> statement-breakpoint
CREATE TABLE `sessions` (
	`id` text PRIMARY KEY NOT NULL,
	`working_directory` text NOT NULL,
	`backend_type` text NOT NULL,
	`model` text NOT NULL,
	`agent_mode` text NOT NULL,
	`created_at` integer NOT NULL,
	`last_activity_at` integer NOT NULL,
	`title` text NOT NULL,
	`main_chain_id` integer NOT NULL,
	`shell_last_seen_index` integer DEFAULT 0 NOT NULL,
	`cogs_json` text DEFAULT '[]' NOT NULL,
	`workspace_dirs` text DEFAULT '[]' NOT NULL,
	`hidden` integer DEFAULT 0 NOT NULL,
	`metadata` text DEFAULT '{}' NOT NULL
);
