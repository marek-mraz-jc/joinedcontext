import { useMemo, useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, queryKeys, unwrap, reasonOf } from "../../api/client";
import { mayRead, usePermissions } from "../../api/permissions";
import { ORG_NAMESPACE } from "../../api/manifest";
import { emailsIn } from "../../pages/apps/members";
import { isPersonId, personLabel, usePersonNames } from "../../api/personNames";
import { Combobox } from "./Combobox";
import type { PickerOption } from "./Combobox";
import { useDebounced } from "./organizationModels";

interface PersonPickerBase {
  id?: string;
  label: string;
  /** Named by a `<label for={id}>` of the page (see `Combobox`). */
  labelled?: boolean;
  /** Person ids already taken (a group's members): not offered again. */
  exclude?: string[];
  disabled?: boolean;
  required?: boolean;
  invalid?: boolean;
  describedBy?: string;
  /** What a surrounding `Field` hands its control: its hint and error ids. */
  "aria-describedby"?: string;
}

export type PersonPickerProps = PersonPickerBase &
  (
    | {
        multiple?: false;
        /** The chosen person's Keycloak user id (ADR-N-061). */
        value: string;
        onChange: (id: string) => void;
      }
    | {
        /** Several people at once; a pasted list of e-mails adds each person it finds (T-5605). */
        multiple: true;
        value: string[];
        onChange: (ids: string[]) => void;
        /** The words of a pasted list that name nobody listed, as typed: the page names them. */
        onRefused: (words: string[]) => void;
      }
  );

/**
 * A person of the organization, picked from the realm's people the server lists to this caller
 * (GET /organization/people, Person read), searched on the server as they type (T-4947). The value
 * is the person's Keycloak user id, never their e-mail (ADR-N-061): an invited person has one from
 * the moment of the invitation. A caller who may not list people pastes an id. The door still
 * decides: a binding or group naming nobody grants nobody.
 */
export function PersonPicker(props: PersonPickerProps): JSX.Element {
  const {
    id,
    label,
    labelled,
    exclude = [],
    disabled,
    required,
    invalid,
    describedBy,
    "aria-describedby": fieldDescribedBy,
  } = props;
  const { t } = useTranslation();
  const permissions = usePermissions(ORG_NAMESPACE);
  const reads = mayRead(permissions.data, "Person") === true;
  const [typed, setTyped] = useState("");
  const search = useDebounced(typed.trim());
  const people = useQuery({
    queryKey: queryKeys.peoplePage(search, 0),
    enabled: reads,
    queryFn: async () =>
      unwrap(await api.GET("/api/v1/organization/people", { params: { query: { max: 100, ...(search ? { search } : {}) } } })),
  });
  const options = useMemo<PickerOption[]>(() => {
    const taken = new Set(exclude);
    return (people.data?.items ?? [])
      .filter((person) => person.enabled && !taken.has(person.id))
      .map((person) => {
        const name = `${person.firstName} ${person.lastName}`.trim();
        return {
          value: person.id,
          label: name || person.email,
          detail: name ? person.email : undefined,
          group: t("picker.person.group"),
        };
      });
  }, [people.data, exclude, t]);
  const chosen = props.multiple ? props.value : props.value ? [props.value] : [];
  const names = usePersonNames(chosen);
  // A chosen person stays visible as a pill even when the current search does not list them.
  const shown = [
    ...options,
    ...chosen
      .filter((id) => !options.some((option) => option.value === id))
      .map((id) => ({ value: id, label: personLabel(id, names), group: t("picker.person.chosen") })),
  ];
  const choose = (ids: string[]) => (props.multiple ? props.onChange(ids) : props.onChange(ids[0] ?? ""));
  // A pasted id names its person; a pasted list of e-mails adds each person the server finds by
  // exactly that address, and names the words that found nobody.
  const create = (text: string) => {
    const id = text.trim().toLowerCase();
    if (!props.multiple) {
      if (isPersonId(id)) props.onChange(id);
      return;
    }
    const { value, onChange, onRefused } = props;
    void idsOf(text, reads).then(({ ids, refused }) => {
      const taken = new Set([...exclude, ...value]);
      onChange([...value, ...ids.filter((found) => !taken.has(found))]);
      onRefused(refused);
    });
  };
  const createLabel = (text: string) => {
    const { emails, invalid: refused } = emailsIn(text);
    if (props.multiple && emails.length + refused.length > 1) return t("picker.person.addList", { count: emails.length });
    if (isPersonId(text.trim().toLowerCase())) return t("picker.person.useId", { id: text.trim().toLowerCase() });
    return emails.length === 1 ? t("picker.person.notInvited", { email: emails[0] }) : t("picker.person.needEmail");
  };
  return (
    <Combobox
      id={id}
      label={label}
      labelled={labelled}
      value={chosen}
      options={shown}
      multiple={props.multiple}
      disabled={disabled}
      required={required}
      invalid={invalid}
      describedBy={describedBy ?? fieldDescribedBy}
      placeholder={t("picker.person.placeholder")}
      loading={reads && people.isLoading}
      failed={people.isError ? { reason: reasonOf(people.error), retry: () => void people.refetch() } : undefined}
      empty={reads ? t("picker.person.none") : t("picker.person.unlisted")}
      onSearch={setTyped}
      onChange={choose}
      create={{ label: createLabel, onCreate: create }}
    />
  );
}

/** The ids of the people a pasted list names: each word is an id, or an e-mail the server lists. Exported for its test. */
export async function idsOf(text: string, reads: boolean): Promise<{ ids: string[]; refused: string[] }> {
  const words = text.split(/[\s,;]+/).filter(Boolean);
  const lookUp = async (word: string): Promise<string | undefined> => {
    const lower = word.toLowerCase();
    if (isPersonId(lower)) return lower;
    if (!reads) return undefined;
    const answer = await api.GET("/api/v1/organization/people", { params: { query: { search: lower, max: 5 } } });
    return answer.data?.items.find((person) => person.email.toLowerCase() === lower)?.id;
  };
  // A few at a time, not one round trip after another (F-R2UI2-010), nor fifty at once at the edge.
  const found: (string | undefined)[] = [];
  for (let at = 0; at < words.length; at += LOOKUPS_AT_ONCE) {
    found.push(...(await Promise.all(words.slice(at, at + LOOKUPS_AT_ONCE).map(lookUp))));
  }
  const ids = found.filter((id): id is string => id !== undefined);
  const refused = words.filter((_, index) => found[index] === undefined);
  return { ids: [...new Set(ids)], refused };
}

const LOOKUPS_AT_ONCE = 6;
